"""SpaceTrace SSH helper: metadata-only, iterative, bounded-memory traversal."""
import datetime
import json
import os
import stat
import sys
import time


def emit(event, **payload):
    def safe(value):
        if isinstance(value, str):
            return value.encode("utf-8", errors="backslashreplace").decode("utf-8")
        if isinstance(value, dict):
            return {key: safe(item) for key, item in value.items()}
        return value
    print(json.dumps(safe(dict(event=event, **payload)), ensure_ascii=True, separators=(",", ":")), flush=True)


def stamp(value):
    try:
        return datetime.datetime.fromtimestamp(value, datetime.timezone.utc).isoformat()
    except (ValueError, OverflowError, OSError):
        return None


def run():
    sys.stdin.reconfigure(encoding="utf-8")
    request_line = sys.stdin.readline(131073)
    if len(request_line) > 131072:
        raise ValueError("Scan request exceeds the permitted size")
    request = json.loads(request_line)
    root = request.get("root")
    if not isinstance(root, str) or not root or "\0" in root:
        raise ValueError("A valid root directory is required")
    root = os.path.abspath(os.path.expanduser(root))
    virtual_roots = ("/proc", "/sys", "/dev")
    if any(root == prefix or root.startswith(prefix + "/") for prefix in virtual_roots):
        raise ValueError("Virtual system filesystems /proc, /sys, and /dev are excluded from storage scans")
    root_stat = os.lstat(root)
    if not stat.S_ISDIR(root_stat.st_mode):
        raise ValueError("The root must be a directory and cannot be a symbolic link")
    next_id = 0
    entries = files = directories = logical = 0
    seen = set()
    last_progress = time.monotonic()

    def node_for(name, relative, parent, metadata, kind):
        nonlocal next_id
        node = dict(id=next_id, parentId=parent, name=name, path=relative, kind=kind,
                    logicalBytes=0, allocatedBytes=0, files=0, directories=0,
                    modified=stamp(metadata.st_mtime), complete=True, shared=False)
        next_id += 1
        return node

    def issue(relative, exc):
        emit("issue", path=relative, kind="permissionDenied" if isinstance(exc, PermissionError) else "ioError",
             message=str(exc)[:8192])

    def open_frame(absolute, node):
        node["complete"] = False
        emit("node", node=node)
        try:
            iterator = os.scandir(absolute)
            complete = True
        except OSError as exc:
            iterator = None
            complete = False
            issue(node["path"], exc)
        return dict(absolute=absolute, node=node, iterator=iterator, complete=complete)

    def emit_partial():
        carry = None
        for frame in reversed(stack):
            snapshot = dict(frame["node"])
            if carry is not None:
                snapshot["logicalBytes"] += carry["logicalBytes"]
                snapshot["allocatedBytes"] += carry["allocatedBytes"]
                snapshot["files"] += carry["files"]
                snapshot["directories"] += carry["directories"] + 1
            snapshot["complete"] = False
            emit("node", node=snapshot)
            carry = snapshot

    root_node = node_for(os.path.basename(root) or root, ".", None, root_stat, "directory")
    stack = [open_frame(root, root_node)]
    directories = 1
    entries = 1
    while stack:
        frame = stack[-1]
        iterator = frame["iterator"]
        try:
            entry = next(iterator) if iterator is not None else None
        except StopIteration:
            entry = None
        except OSError as exc:
            issue(frame["node"]["path"], exc)
            frame["complete"] = False
            entry = None
        if entry is None:
            if iterator is not None:
                iterator.close()
            node = frame["node"]
            node["complete"] = frame["complete"]
            emit("node", node=node)
            stack.pop()
            if stack:
                parent = stack[-1]
                parent["node"]["logicalBytes"] += node["logicalBytes"]
                parent["node"]["allocatedBytes"] += node["allocatedBytes"]
                parent["node"]["files"] += node["files"]
                parent["node"]["directories"] += node["directories"] + 1
                parent["complete"] = parent["complete"] and node["complete"]
            continue
        relative = entry.name if frame["node"]["path"] == "." else frame["node"]["path"] + "/" + entry.name
        entries += 1
        try:
            entry.name.encode("utf-8", errors="strict")
        except UnicodeEncodeError:
            emit("issue", path=relative, kind="invalidNameEncoding", message="Filename contains non-UTF-8 bytes; byte escapes are shown in the report.")
        try:
            metadata = entry.stat(follow_symlinks=False)
        except OSError as exc:
            frame["complete"] = False
            issue(relative, exc)
            continue
        mode = metadata.st_mode
        kind = "directory" if stat.S_ISDIR(mode) else "file" if stat.S_ISREG(mode) else "link" if stat.S_ISLNK(mode) else "special"
        node = node_for(entry.name, relative, frame["node"]["id"], metadata, kind)
        if kind == "directory":
            directories += 1
            if metadata.st_dev != root_stat.st_dev or any(entry.path == prefix for prefix in virtual_roots):
                node["complete"] = False
                frame["complete"] = False
                frame["node"]["directories"] += 1
                emit("node", node=node)
                emit("issue", path=relative, kind="mountBoundarySkipped", message="Another filesystem or a virtual system directory was excluded; scan that filesystem separately.")
            else:
                stack.append(open_frame(entry.path, node))
        else:
            if kind == "file":
                identity = (metadata.st_dev, metadata.st_ino)
                node["shared"] = metadata.st_nlink > 1 and identity in seen
                if metadata.st_nlink > 1:
                    seen.add(identity)
                node["logicalBytes"] = metadata.st_size
                node["allocatedBytes"] = 0 if node["shared"] else metadata.st_blocks * 512
                node["files"] = 1
                files += 1
                logical += metadata.st_size
                frame["node"]["logicalBytes"] += node["logicalBytes"]
                frame["node"]["allocatedBytes"] += node["allocatedBytes"]
                frame["node"]["files"] += 1
            emit("node", node=node)
        now = time.monotonic()
        if entries % 250 == 0 or now - last_progress >= 0.25:
            emit_partial()
            emit("progress", entries=entries, files=files, directories=directories, logicalBytes=logical, currentPath=relative)
            last_progress = now
    emit("progress", entries=entries, files=files, directories=directories, logicalBytes=logical, currentPath=".")
    emit("finished", cancelled=False)


try:
    run()
except (OSError, ValueError, TypeError, KeyError) as error:
    emit("issue", path=".", kind="remoteError", message=str(error)[:8192])
    sys.exit(1)
