# SpaceTrace metadata-only SSH helper. Windows PowerShell 5.1 or newer.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [Console]::OutputEncoding

function Send-Event($Event) {
    [Console]::Out.WriteLine(($Event | ConvertTo-Json -Compress -Depth 6))
    [Console]::Out.Flush()
}
function Send-Issue([string]$Relative, $ErrorRecord) {
    $message = [string]$ErrorRecord.Exception.Message
    if ($message.Length -gt 8192) { $message = $message.Substring(0, 8192) }
    $kind = if ($ErrorRecord.Exception -is [UnauthorizedAccessException]) { 'permissionDenied' } else { 'ioError' }
    Send-Event @{event='issue'; path=$Relative; kind=$kind; message=$message}
}
function New-Node($Info, [string]$Relative, $Parent, [string]$Kind) {
    $node = @{id=$script:nextId; parentId=$Parent; name=$Info.Name; path=$Relative; kind=$Kind;
        logicalBytes=[long]0; allocatedBytes=$null; files=[long]0; directories=[long]0;
        modified=$Info.LastWriteTimeUtc.ToString('o'); complete=$true; shared=$false}
    if (-not $node.name) { $node.name = $Info.FullName }
    $script:nextId++
    return $node
}
function New-Frame($Info, $Node) {
    $Node.complete = $false
    Send-Event @{event='node'; node=$Node}
    $complete = $true
    $iterator = $null
    try { $iterator = $Info.EnumerateFileSystemInfos().GetEnumerator() }
    catch { Send-Issue $Node.path $_; $complete = $false }
    return @{node=$Node; iterator=$iterator; complete=$complete}
}
function Send-Partial($Stack) {
    $carry = $null
    for ($index = $Stack.Count - 1; $index -ge 0; $index--) {
        $snapshot = $Stack[$index].node.Clone()
        if ($null -ne $carry) {
            $snapshot.logicalBytes += $carry.logicalBytes
            $snapshot.files += $carry.files
            $snapshot.directories += $carry.directories + 1
        }
        $snapshot.complete = $false
        Send-Event @{event='node';node=$snapshot}
        $carry = $snapshot
    }
}

try {
    $requestLine = [Console]::In.ReadLine()
    if ($null -eq $requestLine -or $requestLine.Length -gt 131072) { throw 'Invalid scan request' }
    $request = $requestLine | ConvertFrom-Json
    if ($request.root -isnot [string] -or [string]::IsNullOrWhiteSpace($request.root) -or $request.root.Contains([char]0)) { throw 'A valid root directory is required' }
    $rootInfo = [IO.DirectoryInfo]::new([IO.Path]::GetFullPath($request.root))
    if (-not $rootInfo.Exists) { throw 'The root directory does not exist or is inaccessible' }
    if (($rootInfo.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'The root cannot be a junction, symbolic link, or cloud placeholder' }
    $script:nextId = [long]0
    $entries = [long]1
    $files = [long]0
    $directories = [long]1
    $logical = [long]0
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $stack = [Collections.Generic.List[object]]::new()
    $rootNode = New-Node $rootInfo '.' $null 'directory'
    $stack.Add((New-Frame $rootInfo $rootNode))
    while ($stack.Count -gt 0) {
        $frame = $stack[$stack.Count - 1]
        $entry = $null
        try { if ($null -ne $frame.iterator -and $frame.iterator.MoveNext()) { $entry = $frame.iterator.Current } }
        catch { Send-Issue $frame.node.path $_; $frame.complete = $false }
        if ($null -eq $entry) {
            if ($frame.iterator -is [IDisposable]) { $frame.iterator.Dispose() }
            $node = $frame.node
            $node.complete = $frame.complete
            Send-Event @{event='node'; node=$node}
            $stack.RemoveAt($stack.Count - 1)
            if ($stack.Count -gt 0) {
                $parent = $stack[$stack.Count - 1]
                $parent.node.logicalBytes += $node.logicalBytes
                $parent.node.files += $node.files
                $parent.node.directories += $node.directories + 1
                $parent.complete = $parent.complete -and $node.complete
            }
            continue
        }
        $relative = if ($frame.node.path -eq '.') { $entry.Name } else { $frame.node.path + '/' + $entry.Name }
        $entries++
        try {
            $attributes = $entry.Attributes
            $isDirectory = ($attributes -band [IO.FileAttributes]::Directory) -ne 0
            $isReparse = ($attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0
            # Offline / RecallOnOpen / RecallOnDataAccess: inspect metadata only.
            $isCloud = (([long]$attributes -band 0x441000) -ne 0)
            $kind = if ($isReparse -and ($isDirectory -or -not $isCloud)) { 'link' } elseif ($isDirectory) { 'directory' } else { 'file' }
            $node = New-Node $entry $relative $frame.node.id $kind
            if ($isDirectory -and $isReparse) {
                # Reparse tags are unavailable through this metadata-only
                # helper; retain every skipped directory as an explicit issue.
                $node.kind = 'directory'
                $node.complete = $false
                $frame.complete = $false
                $frame.node.directories++
                $directories++
                Send-Event @{event='node';node=$node}
                Send-Event @{event='issue';path=$relative;kind='reparseDirectorySkipped';message='Reparse directory excluded: junctions, symbolic links, and cloud directories are not traversed.'}
                continue
            }
            if ($kind -eq 'directory') {
                $directories++
                $stack.Add((New-Frame $entry $node))
            } else {
                if ($kind -eq 'file') {
                    $node.logicalBytes = [long]$entry.Length
                    $node.files = [long]1
                    $files++
                    $logical += $node.logicalBytes
                    $frame.node.logicalBytes += $node.logicalBytes
                    $frame.node.files++
                    if ($isCloud) { Send-Event @{event='issue';path=$relative;kind='cloudPlaceholder';message='Cloud placeholder: logical size from metadata only; disk allocation is unknown.'} }
                }
                Send-Event @{event='node'; node=$node}
            }
        } catch { Send-Issue $relative $_; $frame.complete = $false }
        if (($entries % 250) -eq 0 -or $timer.ElapsedMilliseconds -ge 250) {
            Send-Partial $stack
            Send-Event @{event='progress';entries=$entries;files=$files;directories=$directories;logicalBytes=$logical;currentPath=$relative}
            $timer.Restart()
        }
    }
    Send-Event @{event='progress';entries=$entries;files=$files;directories=$directories;logicalBytes=$logical;currentPath='.'}
    Send-Event @{event='finished';cancelled=$false}
} catch {
    $message = [string]$_.Exception.Message
    if ($message.Length -gt 8192) { $message = $message.Substring(0,8192) }
    Send-Event @{event='issue';path='.';kind='remoteError';message=$message}
    exit 1
}
