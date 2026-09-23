//! Key-only OpenSSH transport. Helpers receive the selected path through stdin;
//! no user input is interpolated into a remote shell command.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use spacetrace_scanner::ScanEvent;
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    net::IpAddr,
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const MAX_LINE: usize = 256 * 1024;
const MAX_STDERR: usize = 16 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
static TRUST_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub key_path: Option<String>,
    pub root: String,
    pub platform: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostProbe {
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    pub key_line: String,
    pub trusted: bool,
}

fn validate_host(host: &str, port: u16) -> Result<(), String> {
    if port == 0 || host.is_empty() || host.len() > 253 {
        return Err("Enter a valid SSH hostname and a port between 1 and 65535.".into());
    }
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    if !host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    }) {
        return Err(
            "Use a hostname or IP address without shell characters, spaces, or a username.".into(),
        );
    }
    Ok(())
}

fn host_token(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_ascii_lowercase()
    } else {
        format!("[{}]:{}", host.to_ascii_lowercase(), port)
    }
}

fn hidden_command(program: &str) -> Command {
    #[cfg(windows)]
    let mut command = {
        // Prefer the OS-owned OpenSSH executable over an executable in the
        // working directory or an inherited application-specific PATH.
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let binary = std::path::PathBuf::from(system_root)
            .join("System32")
            .join("OpenSSH")
            .join(format!("{program}.exe"));
        Command::new(binary)
    };
    #[cfg(not(windows))]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

struct ProcessContainment {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl Drop for ProcessContainment {
    fn drop(&mut self) {
        // The unnamed job handle is owned only by this process. Windows also
        // closes it if SpaceTrace crashes or exits before its worker can stop.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

struct ContainedProcess {
    child: std::process::Child,
    job: ProcessContainment,
}

fn contain(child: &mut std::process::Child) -> Result<ProcessContainment, String> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        let result = (|| {
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let guard = ProcessContainment { handle };
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if unsafe {
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const std::ffi::c_void,
                    std::mem::size_of_val(&limits) as u32,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            if unsafe { AssignProcessToJobObject(handle, child.as_raw_handle()) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(guard)
        })();
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        result.map_err(|error| {
            format!("The SSH process could not be safely contained and was stopped: {error}")
        })
    }
    #[cfg(not(windows))]
    {
        let _ = child;
        Ok(ProcessContainment {})
    }
}

fn spawn_contained(mut command: Command) -> Result<ContainedProcess, String> {
    let mut child = command.spawn().map_err(|_| {
        "Windows OpenSSH Client is required. Install it in Windows Optional Features.".to_string()
    })?;
    let job = contain(&mut child)?;
    Ok(ContainedProcess { child, job })
}

fn read_capped(mut reader: impl Read, limit: usize) -> Result<(Vec<u8>, bool), String> {
    let mut result = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut truncated = false;
    loop {
        let count = reader
            .read(&mut chunk)
            .map_err(|_| "Could not read SSH output.".to_string())?;
        if count == 0 {
            break;
        }
        let keep = count.min(limit.saturating_sub(result.len()));
        result.extend_from_slice(&chunk[..keep]);
        truncated |= keep < count;
    }
    Ok((result, truncated))
}

fn small_process(command: Command, input: Vec<u8>, timeout: Duration) -> Result<Vec<u8>, String> {
    let ContainedProcess {
        mut child,
        job: _containment,
    } = spawn_contained(command)?;
    let stdout = child.stdout.take().ok_or("SSH stdout is unavailable")?;
    let stderr = child.stderr.take().ok_or("SSH stderr is unavailable")?;
    let stdin = child.stdin.take().ok_or("SSH stdin is unavailable")?;
    thread::spawn(move || {
        let _ = { stdin }.write_all(&input);
    });
    let (out_tx, out_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = out_tx.send(read_capped(stdout, 64 * 1024));
    });
    thread::spawn(move || {
        let _ = err_tx.send(read_capped(stderr, MAX_STDERR));
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(30)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("SSH host verification timed out.".into());
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Could not monitor SSH host verification.".into());
            }
        }
    };
    let (output, truncated) = out_rx
        .recv_timeout(Duration::from_secs(1))
        .map_err(|_| "SSH output did not close".to_string())??;
    let (errors, _) = err_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or(Ok((Vec::new(), false)))?;
    if !status.success() {
        return Err(format!(
            "SSH host verification failed. {}",
            clean_text(&String::from_utf8_lossy(&errors))
        ));
    }
    if truncated {
        return Err("SSH host verification returned excessive output.".into());
    }
    Ok(output)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    algorithm: String,
    encoded: String,
}

fn parse_key_line(line: &str, expected_host: &str) -> Result<Key, String> {
    if line.len() > 16384 || line.contains('\n') || line.contains('\r') || line.contains('\0') {
        return Err("Invalid SSH host key.".into());
    }
    let fields: Vec<_> = line.split_whitespace().collect();
    if fields.len() != 3 || !fields[0].eq_ignore_ascii_case(expected_host) {
        return Err("The host key does not match the requested host and port.".into());
    }
    if !matches!(
        fields[1],
        "ssh-ed25519"
            | "ecdsa-sha2-nistp256"
            | "ecdsa-sha2-nistp384"
            | "ecdsa-sha2-nistp521"
            | "ssh-rsa"
    ) {
        return Err("Unsupported SSH host key algorithm.".into());
    }
    let decoded = STANDARD
        .decode(fields[2])
        .map_err(|_| "Invalid SSH host key encoding".to_string())?;
    if decoded.len() < 8 || decoded.len() > 8192 {
        return Err("Invalid SSH host key length.".into());
    }
    let length = u32::from_be_bytes(decoded[..4].try_into().unwrap()) as usize;
    if length > decoded.len() - 4 || decoded.get(4..4 + length) != Some(fields[1].as_bytes()) {
        return Err("SSH host key payload does not match its algorithm.".into());
    }
    Ok(Key {
        algorithm: fields[1].into(),
        encoded: fields[2].into(),
    })
}

fn existing_keys(path: &Path, token: &str) -> Result<Vec<Key>, String> {
    let content = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("Could not read SpaceTrace's trusted SSH hosts.".into()),
    };
    if content.len() > 4 * 1024 * 1024 {
        return Err("The trusted-host file is unexpectedly large.".into());
    }
    content
        .lines()
        .filter(|line| {
            line.split_whitespace()
                .next()
                .is_some_and(|field| field.eq_ignore_ascii_case(token))
        })
        .map(|line| parse_key_line(line, token))
        .collect()
}

fn fingerprint(line: &str) -> Result<String, String> {
    let mut command = hidden_command("ssh-keygen");
    command.args(["-l", "-E", "sha256", "-f", "-"]);
    let output = small_process(
        command,
        format!("{line}\n").into_bytes(),
        Duration::from_secs(10),
    )?;
    String::from_utf8_lossy(&output)
        .split_whitespace()
        .find(|field| field.starts_with("SHA256:"))
        .map(str::to_owned)
        .ok_or_else(|| "OpenSSH did not return a valid host fingerprint.".into())
}

pub fn probe(host: &str, port: u16, known_hosts: &Path) -> Result<HostProbe, String> {
    validate_host(host, port)?;
    let token = host_token(host, port);
    let existing = existing_keys(known_hosts, &token)?;
    let mut command = hidden_command("ssh-keyscan");
    command.args([
        "-T",
        "7",
        "-p",
        &port.to_string(),
        "-t",
        "ed25519,ecdsa,rsa",
        host,
    ]);
    let output = small_process(command, Vec::new(), Duration::from_secs(15))?;
    let mut keys = Vec::new();
    for line in String::from_utf8_lossy(&output).lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        keys.push(parse_key_line(line, &token)?);
    }
    if keys.is_empty() {
        return Err("The host returned no supported SSH host key.".into());
    }
    keys.sort_by_key(|key| match key.algorithm.as_str() {
        "ssh-ed25519" => 0,
        "ssh-rsa" => 2,
        _ => 1,
    });
    let trusted = !existing.is_empty();
    let selected = if trusted {
        keys.iter().find(|key| existing.contains(key)).ok_or("The SSH host key has changed. Connection refused. Verify the change with the device administrator and remove the old entry from SpaceTrace's known_hosts file only after verification.")?
    } else {
        &keys[0]
    };
    let key_line = format!("{token} {} {}", selected.algorithm, selected.encoded);
    Ok(HostProbe {
        host: host.into(),
        port,
        fingerprint: fingerprint(&key_line)?,
        key_line,
        trusted,
    })
}

pub fn trust(host: &str, port: u16, key_line: &str, known_hosts: &Path) -> Result<(), String> {
    validate_host(host, port)?;
    let token = host_token(host, port);
    let key = parse_key_line(key_line, &token)?;
    // ssh-keygen also validates the complete cryptographic payload, not just
    // its base64 wrapper and algorithm prefix.
    fingerprint(key_line)?;
    let _lock = TRUST_LOCK
        .lock()
        .map_err(|_| "Trusted-host storage is unavailable".to_string())?;
    let existing = existing_keys(known_hosts, &token)?;
    if existing.contains(&key) {
        return Ok(());
    }
    if !existing.is_empty() {
        return Err(
            "A different host key is already trusted. SpaceTrace will not replace it.".into(),
        );
    }
    if let Some(parent) = known_hosts.parent() {
        fs::create_dir_all(parent)
            .map_err(|_| "Could not create trusted-host storage".to_string())?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(known_hosts)
        .map_err(|_| "Could not save the trusted SSH host".to_string())?;
    writeln!(file, "\n{token} {} {}", key.algorithm, key.encoded)
        .map_err(|_| "Could not save the trusted SSH host".to_string())?;
    file.sync_all()
        .map_err(|_| "Could not finish saving the trusted SSH host".to_string())
}

fn validate_request(request: &RemoteRequest) -> Result<(), String> {
    validate_host(&request.host, request.port)?;
    if request.username.is_empty()
        || request.username.len() > 128
        || request.username.starts_with('-')
        || !request.username.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'\\' | b'@')
        })
    {
        return Err("Use an SSH account name, DOMAIN\\user, or user@domain without spaces or shell characters.".into());
    }
    if !matches!(request.platform.as_str(), "linux" | "windows") {
        return Err("Choose Linux or Windows for the remote platform.".into());
    }
    if request.root.is_empty() || request.root.len() > 32768 || request.root.contains('\0') {
        return Err("Enter a valid remote starting folder.".into());
    }
    if let Some(key) = request.key_path.as_ref().filter(|value| !value.is_empty()) {
        if !Path::new(key).is_absolute() || !Path::new(key).is_file() {
            return Err("Choose an existing private-key file using its absolute path.".into());
        }
    }
    Ok(())
}

fn remote_command(platform: &str) -> String {
    if platform == "linux" {
        format!(
            "python3 -u -c \"import base64;exec(base64.b64decode('{}'))\"",
            STANDARD.encode(include_bytes!("../helpers/scan_linux.py"))
        )
    } else {
        // Keep below cmd.exe's 8191-character command limit. The fixed loader
        // receives the shipped helper as base64 on stdin, followed by its JSON
        // request. Neither the helper nor user input is parsed by cmd.exe.
        let loader = "[Console]::InputEncoding=[Text.UTF8Encoding]::new($false);& ([ScriptBlock]::Create([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String([Console]::In.ReadLine()))))";
        let bytes: Vec<u8> = loader.encode_utf16().flat_map(u16::to_le_bytes).collect();
        format!(
            "powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand {}",
            STANDARD.encode(bytes)
        )
    }
}

fn ssh_command(request: &RemoteRequest, known_hosts: &Path) -> Command {
    let mut command = hidden_command("ssh");
    command.args(["-F", "none", "-T"]);
    for option in [
        "BatchMode=yes",
        "PreferredAuthentications=publickey",
        "PasswordAuthentication=no",
        "KbdInteractiveAuthentication=no",
        "HostbasedAuthentication=no",
        "NumberOfPasswordPrompts=0",
        "StrictHostKeyChecking=yes",
        "ConnectTimeout=10",
        "ConnectionAttempts=1",
        "ServerAliveInterval=15",
        "ServerAliveCountMax=2",
        "ClearAllForwardings=yes",
        "ForwardAgent=no",
        "ForwardX11=no",
        "PermitLocalCommand=no",
        "ProxyCommand=none",
        "ProxyJump=none",
        "ControlMaster=no",
        "ControlPath=none",
        "UpdateHostKeys=no",
        "LogLevel=ERROR",
    ] {
        command.args(["-o", option]);
    }
    let known_path = known_hosts
        .to_string_lossy()
        .replace('\\', "/")
        .replace('%', "%%");
    command
        .arg("-o")
        .arg(format!("UserKnownHostsFile=\"{known_path}\""));
    command.args([
        "-o",
        if cfg!(windows) {
            "GlobalKnownHostsFile=NUL"
        } else {
            "GlobalKnownHostsFile=/dev/null"
        },
    ]);
    if let Some(key) = request.key_path.as_ref().filter(|value| !value.is_empty()) {
        command.args(["-o", "IdentitiesOnly=yes", "-i", key]);
    }
    command.args([
        "-p",
        &request.port.to_string(),
        "-l",
        &request.username,
        &request.host,
    ]);
    command.arg(remote_command(&request.platform));
    command
}

fn clean_text(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .take(4096)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn valid_relative(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 32768
        && !path.contains('\0')
        && !path.starts_with('/')
        && path.split('/').all(|part| part != ".." && !part.is_empty())
}

fn parse_event(line: &str) -> Result<ScanEvent, String> {
    let event: ScanEvent = serde_json::from_str(line).map_err(|_| "The remote helper returned invalid scan data. Check the selected platform and its Python/PowerShell installation.".to_string())?;
    let valid = match &event {
        ScanEvent::Node { node } => {
            node.id >= 0
                && node
                    .parent_id
                    .is_none_or(|parent| parent >= 0 && parent < node.id)
                && ((node.id == 0
                    && node.parent_id.is_none()
                    && node.path == "."
                    && node.kind == "directory")
                    || (node.id > 0 && node.parent_id.is_some() && node.path != "."))
                && !node.name.is_empty()
                && node.name.len() <= 32768
                && !node.name.contains('\0')
                && valid_relative(&node.path)
                && matches!(
                    node.kind.as_str(),
                    "directory" | "file" | "link" | "special"
                )
                && node
                    .modified
                    .as_ref()
                    .is_none_or(|value| value.len() <= 128)
        }
        ScanEvent::Issue {
            path,
            kind,
            message,
        } => valid_relative(path) && kind.len() <= 128 && message.len() <= 32768,
        ScanEvent::Progress { current_path, .. } => valid_relative(current_path),
        ScanEvent::Finished { .. } => true,
    };
    if valid {
        Ok(event)
    } else {
        Err("The remote helper returned invalid paths or node fields.".into())
    }
}

fn read_line_capped(reader: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut bytes = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .map_err(|_| "Could not read SSH scan data".to_string())?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(available.len(), |position| position + 1);
        if bytes.len() + count > MAX_LINE {
            return Err("The SSH helper returned an oversized event.".into());
        }
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if newline.is_some() {
            break;
        }
    }
    String::from_utf8(bytes)
        .map(|line| Some(line.trim_end_matches(['\r', '\n']).to_owned()))
        .map_err(|_| "The SSH helper returned invalid UTF-8".into())
}

pub fn scan(
    request: &RemoteRequest,
    known_hosts: &Path,
    cancel: &AtomicBool,
    mut emit: impl FnMut(ScanEvent) -> Result<(), String>,
) -> Result<(), String> {
    validate_request(request)?;
    if existing_keys(known_hosts, &host_token(&request.host, request.port))?.is_empty() {
        return Err("Verify and trust this host's SSH fingerprint before scanning.".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return emit(ScanEvent::Finished { cancelled: true });
    }
    let child = spawn_contained(ssh_command(request, known_hosts))?;
    let mut input = Vec::new();
    if request.platform == "windows" {
        input.extend_from_slice(
            STANDARD
                .encode(include_bytes!("../helpers/scan_windows.ps1"))
                .as_bytes(),
        );
        input.push(b'\n');
    }
    input.extend_from_slice(
        &serde_json::to_vec(&serde_json::json!({ "root": request.root }))
            .map_err(|error| error.to_string())?,
    );
    stream_process(
        child,
        input,
        request.key_path.as_deref(),
        cancel,
        IDLE_TIMEOUT,
        emit,
    )
}

fn stream_process(
    process: ContainedProcess,
    input: Vec<u8>,
    key_path: Option<&str>,
    cancel: &AtomicBool,
    idle_timeout: Duration,
    mut emit: impl FnMut(ScanEvent) -> Result<(), String>,
) -> Result<(), String> {
    let ContainedProcess {
        mut child,
        job: _containment,
    } = process;
    let mut stdin = child.stdin.take().ok_or("SSH stdin is unavailable")?;
    let stdout = child.stdout.take().ok_or("SSH stdout is unavailable")?;
    let stderr = child.stderr.take().ok_or("SSH stderr is unavailable")?;
    thread::spawn(move || {
        let _ = stdin.write_all(&input).and_then(|_| stdin.write_all(b"\n"));
    });
    let (tx, rx) = mpsc::sync_channel::<Result<String, String>>(64);
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_line_capped(&mut reader) {
                Ok(Some(line)) => {
                    if tx.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = tx.send(Err(error));
                    break;
                }
            }
        }
    });
    let (err_tx, err_rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = err_tx.send(read_capped(stderr, MAX_STDERR));
    });
    let mut last_output = Instant::now();
    let mut finished = false;
    let mut stdout_closed = false;
    let result = loop {
        if cancel.load(Ordering::Relaxed) {
            break emit(ScanEvent::Finished { cancelled: true });
        }
        if last_output.elapsed() > idle_timeout {
            break Err(
                "The remote scan stopped responding for 90 seconds. Partial results were retained."
                    .into(),
            );
        }
        if !stdout_closed {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Ok(line)) => {
                    if finished {
                        break Err("The remote helper sent data after its completion event.".into());
                    }
                    match parse_event(&line) {
                        Ok(ScanEvent::Finished { cancelled: false }) => finished = true,
                        Ok(ScanEvent::Finished { cancelled: true }) => {
                            break Err("The remote helper unexpectedly cancelled its scan.".into())
                        }
                        Ok(event) => {
                            if let Err(error) = emit(event) {
                                break Err(error);
                            }
                        }
                        Err(error) => break Err(error),
                    }
                    last_output = Instant::now();
                }
                Ok(Err(error)) => break Err(error),
                Err(mpsc::RecvTimeoutError::Disconnected) => stdout_closed = true,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        } else {
            thread::sleep(Duration::from_millis(25));
        }
        match child.try_wait() {
            Ok(Some(status)) if stdout_closed => {
                if status.success() && finished {
                    break emit(ScanEvent::Finished { cancelled: false });
                }
                let stderr = err_rx
                    .recv_timeout(Duration::from_secs(1))
                    .ok()
                    .and_then(Result::ok)
                    .map(|(bytes, _)| clean_text(&String::from_utf8_lossy(&bytes)))
                    .unwrap_or_default();
                let stderr = if let Some(key) = key_path {
                    if key.is_empty() {
                        stderr
                    } else {
                        stderr.replace(key, "[private key]")
                    }
                } else {
                    stderr
                };
                break Err(if stderr.is_empty() {
                    "SSH scan ended before a complete report was received. Linux requires python3; Windows requires Windows PowerShell.".into()
                } else {
                    format!("SSH scan failed: {stderr}")
                });
            }
            Ok(_) => {}
            Err(_) => break Err("Could not monitor the SSH scan process.".into()),
        }
    };
    // Killing a disconnected SSH session also ends its inline helper; no agent,
    // credentials, scripts, or scan reports are installed on the remote device.
    let _ = child.kill();
    let _ = child.wait();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_line(host: &str) -> String {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&11_u32.to_be_bytes());
        bytes.extend_from_slice(b"ssh-ed25519");
        bytes.extend_from_slice(&32_u32.to_be_bytes());
        bytes.extend_from_slice(&[7_u8; 32]);
        format!("{host} ssh-ed25519 {}", STANDARD.encode(bytes))
    }

    #[test]
    fn rejects_shell_and_option_injection() {
        for host in [
            "-oProxyCommand=evil",
            "host;whoami",
            "host name",
            "user@host",
            "host\n",
            "$(id)",
            "host/other",
            "[::1]",
        ] {
            assert!(validate_host(host, 22).is_err(), "{host}");
        }
        for host in [
            "server",
            "server.example",
            "192.168.1.1",
            "::1",
            "2001:db8::1",
        ] {
            assert!(validate_host(host, 22).is_ok());
        }
        assert!(validate_host("server", 0).is_err());
    }

    #[test]
    fn binds_keys_to_exact_host_and_port() {
        assert!(parse_key_line(&key_line("[server]:2222"), "[server]:2222").is_ok());
        assert!(parse_key_line(&key_line("server"), "[server]:2222").is_err());
        assert!(parse_key_line(&key_line("other"), "server").is_err());
        assert!(parse_key_line(&(key_line("server") + "\nother ssh-rsa AAAA"), "server").is_err());
        assert!(parse_key_line("server ssh-ed25519 AAAA", "server").is_err());
    }

    #[test]
    fn rejects_malformed_protocol_and_parent_escape() {
        assert!(parse_event("login banner").is_err());
        assert!(parse_event(r#"{"event":"progress","entries":-1,"files":0,"directories":0,"logicalBytes":0,"currentPath":"."}"#).is_err());
        assert!(parse_event(
            r#"{"event":"issue","path":"../secret","kind":"ioError","message":"x"}"#
        )
        .is_err());
        assert!(parse_event(r#"{"event":"finished","cancelled":false}"#).is_ok());
        assert!(parse_event(r#"{"event":"progress","entries":1,"files":0,"directories":1,"logicalBytes":0,"currentPath":"."}"#).is_ok());
    }

    #[test]
    fn caps_lines_before_allocating_unbounded_data() {
        let data = vec![b'x'; MAX_LINE + 1];
        assert!(read_line_capped(&mut BufReader::new(&data[..])).is_err());
        let mut reader = BufReader::new(&b"one\r\ntwo\n"[..]);
        assert_eq!(read_line_capped(&mut reader).unwrap(), Some("one".into()));
        assert_eq!(read_line_capped(&mut reader).unwrap(), Some("two".into()));
        assert_eq!(read_line_capped(&mut reader).unwrap(), None);
    }

    #[test]
    fn request_data_never_enters_remote_command() {
        let request = RemoteRequest {
            host: "server".into(),
            port: 22,
            username: "user".into(),
            key_path: None,
            root: "$(touch /bad);'\"".into(),
            platform: "linux".into(),
        };
        assert!(validate_request(&request).is_ok());
        let command = ssh_command(&request, Path::new("known_hosts"));
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>();
        assert!(!args.iter().any(|value| value.contains(&request.root)));
        assert!(args
            .iter()
            .any(|value| value == "StrictHostKeyChecking=yes"));
        assert!(args.iter().any(|value| value == "BatchMode=yes"));
        let mut invalid = request;
        invalid.username = "user;id".into();
        assert!(validate_request(&invalid).is_err());
        invalid.username = "DOMAIN\\user".into();
        assert!(validate_request(&invalid).is_ok());
        invalid.username = "user@example.com".into();
        assert!(validate_request(&invalid).is_ok());
    }

    #[test]
    fn windows_loader_stays_below_cmd_command_limit() {
        let command = remote_command("windows");
        assert!(command.len() < 8191);
        assert!(command
            .starts_with("powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand "));
    }

    #[test]
    fn trusted_keys_are_idempotent_and_never_silently_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("known hosts");
        let original = key_line("fixture.example");
        trust("fixture.example", 22, &original, &path).unwrap();
        let before = fs::read_to_string(&path).unwrap();
        trust("fixture.example", 22, &original, &path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        let changed = original.replace("BwcH", "CAgI");
        assert_ne!(changed, original);
        assert!(trust("fixture.example", 22, &changed, &path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        assert!(trust("other.example", 22, &original, &path).is_err());
    }

    #[test]
    fn cancellation_before_connect_returns_finished_without_network() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("known_hosts");
        fs::write(&path, key_line("fixture.example")).unwrap();
        let request = RemoteRequest {
            host: "fixture.example".into(),
            port: 22,
            username: "user".into(),
            key_path: None,
            root: "/".into(),
            platform: "linux".into(),
        };
        let cancel = AtomicBool::new(true);
        let mut cancelled = false;
        scan(&request, &path, &cancel, |event| {
            cancelled = matches!(event, ScanEvent::Finished { cancelled: true });
            Ok(())
        })
        .unwrap();
        assert!(cancelled);
    }

    #[test]
    #[cfg(windows)]
    fn host_probe_process_timeout_is_bounded() {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x08000000);
        let started = Instant::now();
        assert!(small_process(command, Vec::new(), Duration::from_millis(100)).is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(windows)]
    fn fixture_process(script: &str) -> ContainedProcess {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x08000000);
        spawn_contained(command).unwrap()
    }

    #[test]
    #[cfg(windows)]
    fn cancellation_terminates_a_process_with_blocked_stdout() {
        let child = fixture_process("[Console]::Out.WriteLine('{\"event\":\"progress\",\"entries\":1,\"files\":0,\"directories\":1,\"logicalBytes\":0,\"currentPath\":\".\"}');Start-Sleep -Seconds 30");
        let cancel = AtomicBool::new(false);
        let mut events = Vec::new();
        let started = Instant::now();
        stream_process(child, Vec::new(), None, &cancel, IDLE_TIMEOUT, |event| {
            if matches!(event, ScanEvent::Progress { .. }) {
                cancel.store(true, Ordering::Relaxed);
            }
            events.push(event);
            Ok(())
        })
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(events.len(), 2);
        assert!(matches!(events[1], ScanEvent::Finished { cancelled: true }));
    }

    #[test]
    #[cfg(windows)]
    fn idle_timeout_terminates_a_silent_process() {
        let child = fixture_process("Start-Sleep -Seconds 30");
        let started = Instant::now();
        let result = stream_process(
            child,
            Vec::new(),
            None,
            &AtomicBool::new(false),
            Duration::from_millis(100),
            |_| Ok(()),
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    #[cfg(windows)]
    fn malformed_tail_keeps_previous_events_without_reporting_success() {
        let child = fixture_process("[Console]::Out.WriteLine('{\"event\":\"progress\",\"entries\":1,\"files\":0,\"directories\":1,\"logicalBytes\":0,\"currentPath\":\".\"}');[Console]::Out.WriteLine('broken protocol')");
        let mut count = 0;
        let result = stream_process(
            child,
            Vec::new(),
            None,
            &AtomicBool::new(false),
            IDLE_TIMEOUT,
            |event| {
                assert!(!matches!(event, ScanEvent::Finished { .. }));
                count += 1;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(count, 1);
    }

    #[test]
    #[cfg(windows)]
    fn dropping_job_guard_terminates_its_child() {
        let ContainedProcess { mut child, job } = fixture_process("Start-Sleep -Seconds 30");
        drop(job);
        let started = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if started.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Closing the Windows job failed to terminate its child");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}
