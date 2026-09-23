import { useEffect, useState, type FormEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { ArrowRight, Check, FolderOpen, HardDrive, Info, Loader2, LockKeyhole, Monitor, Network, Search, ShieldCheck, Terminal, TriangleAlert } from 'lucide-react';
import type { ScanInfo, ScanRequest, SourceKind } from '../contracts';
import Modal, { native, errorText } from './Modal';
const sourceLabel = { local: 'This device', share: 'Network share', ssh: 'SSH connection' };
const sourceIcon = { local: HardDrive, share: Network, ssh: Terminal };
export default function SourceDialog({ initialKind, initialRoot, onClose, onStarted }: { initialKind: SourceKind; initialRoot: string; onClose: () => void; onStarted: (scan: ScanInfo) => void }) {
  const [kind, setKind] = useState<SourceKind>(initialKind);
  const [root, setRoot] = useState(initialKind === 'local' ? initialRoot : initialKind === 'ssh' ? '/' : '');
  const [host, setHost] = useState('');
  const [port, setPort] = useState('22');
  const [username, setUsername] = useState('');
  const [keyPath, setKeyPath] = useState('');
  const [platform, setPlatform] = useState<'linux' | 'windows'>('linux');
  const [probe, setProbe] = useState<{ fingerprint: string; keyLine: string; trusted: boolean } | null>(null);
  const [busy, setBusy] = useState('');
  const [error, setError] = useState('');
  useEffect(() => { setProbe(null); }, [host, port]);
  const pickKind = (value: SourceKind) => { setKind(value); setRoot(value === 'local' ? initialRoot : value === 'ssh' ? platform === 'linux' ? '/' : 'C:\\' : ''); setError(''); };
  const choose = async (key = false) => {
    try {
      const path = await open({ directory: !key, multiple: false, title: key ? 'Select SSH private key' : 'Choose a folder to scan' });
      if (typeof path === 'string') key ? setKeyPath(path) : setRoot(path);
    } catch (error) { setError(errorText(error)); }
  };
  const verify = async () => {
    setBusy('verify'); setError('');
    try { setProbe(await invoke('probe_host', { host: host.trim(), port: Number(port) })); }
    catch (error) { setError(errorText(error)); }
    finally { setBusy(''); }
  };
  const trust = async () => {
    if (!probe) return;
    setBusy('trust'); setError('');
    try { await invoke('trust_host', { host: host.trim(), port: Number(port), keyLine: probe.keyLine }); setProbe({ ...probe, trusted: true }); }
    catch (error) { setError(errorText(error)); }
    finally { setBusy(''); }
  };
  const start = async (event: FormEvent) => {
    event.preventDefault();
    if (!allowed) return;
    setBusy('start'); setError('');
    const request: ScanRequest = { kind, root: root.trim(), ...(kind === 'ssh' ? { host: host.trim(), port: Number(port), username: username.trim(), keyPath: keyPath.trim() || undefined, platform } : {}) };
    try { onStarted(await invoke<ScanInfo>('start_scan', { request })); }
    catch (error) { setError(errorText(error)); setBusy(''); }
  };
  const allowed = native && root.trim() && !busy && (kind !== 'ssh' || (host.trim() && username.trim() && probe?.trusted && Number(port) > 0 && Number(port) < 65536));
  return <Modal title="Where should we look?" onClose={busy === 'start' ? () => {} : onClose}>
    <p className="modal-intro">Choose a source. We’ll trace the space, folder by folder.</p>
    <div className="source-tabs" role="tablist" aria-label="Scan source">{(['local', 'share', 'ssh'] as SourceKind[]).map(value => { const Icon = sourceIcon[value]; return <button role="tab" aria-selected={kind === value} key={value} onClick={() => pickKind(value)} disabled={!!busy} className={kind === value ? 'active' : ''}><Icon size={18} />{sourceLabel[value]}</button>; })}</div>
    <form onSubmit={start}>
      {kind === 'ssh' && <><div className="form-row"><label className="field grow">Host<input value={host} onChange={event => setHost(event.target.value)} placeholder="server.example.com" autoComplete="off" required disabled={!!busy} /></label><label className="field port">Port<input type="number" min="1" max="65535" value={port} onChange={event => setPort(event.target.value)} required disabled={!!busy} /></label></div><div className="form-row"><label className="field grow">Username<input value={username} onChange={event => setUsername(event.target.value)} placeholder="Your SSH username" autoComplete="off" required disabled={!!busy} /></label><label className="field grow">Remote system<select value={platform} onChange={event => { const value = event.target.value as 'linux' | 'windows'; setPlatform(value); setRoot(value === 'linux' ? '/' : 'C:\\'); }} disabled={!!busy}><option value="linux">Linux</option><option value="windows">Windows</option></select></label></div><label className="field">Private key <span className="optional">optional · uses SSH agent if empty</span><div className="input-action"><input value={keyPath} onChange={event => setKeyPath(event.target.value)} placeholder="Use your SSH agent or select a key" disabled={!!busy} /><button type="button" className="button secondary" disabled={!native || !!busy} onClick={() => choose(true)}><FolderOpen size={16} />Browse</button></div></label></>}
      <label className="field">{kind === 'ssh' ? 'Remote starting folder' : kind === 'share' ? 'Share path' : 'Starting folder'}<div className="input-action"><input value={root} onChange={event => setRoot(event.target.value)} placeholder={kind === 'share' ? '\\\\server\\share\\folder' : kind === 'ssh' ? platform === 'linux' ? '/home' : 'C:\\Users' : 'Choose a drive or folder'} required disabled={!!busy} />{kind !== 'ssh' && <button type="button" className="button secondary" onClick={() => choose()} disabled={!native || !!busy}><FolderOpen size={16} />Browse</button>}</div></label>
      {kind === 'share' && <p className="field-help"><Info size={15} />Uses your existing Windows permissions and connected shares.</p>}
      {kind === 'local' && <p className="field-help"><Info size={15} />Subfolders are included. Files are read for metadata only.</p>}
      {kind === 'ssh' && <div className={`host-verification ${probe?.trusted ? 'verified' : ''}`}><div className="verification-title"><ShieldCheck size={19} /><strong>{probe?.trusted ? 'Host identity verified' : 'Verify the server’s identity'}</strong>{!probe && <button type="button" className="button small secondary" onClick={verify} disabled={!native || !!busy || !host.trim() || !(Number(port) > 0 && Number(port) < 65536)}>{busy === 'verify' ? <Loader2 className="spin" size={14} /> : null}Verify host</button>}</div>{probe ? <><code>{probe.fingerprint}</code>{!probe.trusted && <><p>Compare this fingerprint with a trusted source before saving it.</p><button type="button" className="button small secondary" onClick={trust} disabled={!!busy}>{busy === 'trust' ? <Loader2 size={14} className="spin" /> : <Check size={14} />}Trust this host</button></>}</> : <p>Your connection uses SSH keys and checks the server before scanning.</p>}<p className="helper-note">A temporary scanner runs on the remote device. Your account needs permission to execute it.</p></div>}
      {!native && <div className="notice"><Monitor size={17} /><span>Folder access is available in the installed Windows app. This browser view is a preview of the interface.</span></div>}
      {error && <div className="error-notice" role="alert"><TriangleAlert size={17} /><span>{error}</span></div>}
      <div className="modal-footer"><span><LockKeyhole size={14} />Your files stay where they are</span><button type="submit" className="button primary" disabled={!allowed}>{busy === 'start' ? <Loader2 size={17} className="spin" /> : <Search size={17} />}{busy === 'start' ? 'Starting scan…' : 'Start scanning'}<ArrowRight size={16} /></button></div>
    </form>
  </Modal>;
}
