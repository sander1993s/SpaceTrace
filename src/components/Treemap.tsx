import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowUpRight, File, Folder, Map } from 'lucide-react';
import type { NodeInfo } from '../contracts';
import { byteValue, formatBytes, layoutTreemap, nodeColor } from '../lib/treemap';

interface Props {
  nodes: NodeInfo[];
  metric: 'logical' | 'allocated';
  hovered: number | null;
  onHover: (id: number | null) => void;
  onOpen: (node: NodeInfo) => void;
  running: boolean;
}

export default function Treemap({ nodes, metric, hovered, onHover, onOpen, running }: Props) {
  const container = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: 800, height: 360 });
  useEffect(() => {
    if (!container.current) return;
    const observer = new ResizeObserver(([entry]) => setSize({ width: entry.contentRect.width, height: entry.contentRect.height }));
    observer.observe(container.current);
    return () => observer.disconnect();
  }, []);
  const tiles = useMemo(() => layoutTreemap(nodes.map(node => ({ data: node, value: byteValue(metric === 'logical' ? node.logicalBytes : node.allocatedBytes) })), size.width, size.height), [nodes, metric, size]);
  return <div className="treemap" ref={container} aria-label="Storage treemap">
    {!tiles.length && <div className="map-empty"><Map size={35} strokeWidth={1.2} /><strong>{running ? 'Mapping your space…' : 'No sized items to display'}</strong><span>{running ? 'Folders appear as the scan discovers them.' : metric === 'allocated' ? 'Allocated size is unavailable or zero for these items.' : 'This folder is empty, inaccessible, or its items are filtered out.'}</span></div>}
    {tiles.map(({ data: node, x, y, width, height }) => {
      const small = width < 95 || height < 66;
      const tiny = width < 46 || height < 31;
      return <button key={node.id} className={`map-tile ${hovered === node.id ? 'is-hovered' : ''} ${small ? 'small' : ''} ${tiny ? 'tiny' : ''}`} style={{ left: x + 2, top: y + 2, width: Math.max(0, width - 4), height: Math.max(0, height - 4), background: nodeColor(node.name) }} onMouseEnter={() => onHover(node.id)} onMouseLeave={() => onHover(null)} onFocus={() => onHover(node.id)} onBlur={() => onHover(null)} onClick={() => onOpen(node)} title={`${node.name} · ${formatBytes(metric === 'logical' ? node.logicalBytes : node.allocatedBytes)}${node.complete ? '' : running ? ' · still scanning' : ' · incomplete measurement'}`} aria-label={`${node.kind === 'directory' ? 'Open folder' : 'Select file'} ${node.name}, ${formatBytes(metric === 'logical' ? node.logicalBytes : node.allocatedBytes)}`}>
        {!tiny && <><span className="tile-name">{!small && (node.kind === 'directory' ? <Folder size={16} /> : <File size={16} />)}<span>{node.name}</span></span><span className="tile-size">{formatBytes(metric === 'logical' ? node.logicalBytes : node.allocatedBytes)}</span>{!small && node.kind === 'directory' && <ArrowUpRight className="tile-arrow" size={17} />}{!small && height > 110 && <span className="tile-detail">{node.kind === 'directory' ? `${node.files.toLocaleString()} files` : 'File'}{!node.complete && (running ? ' · counting' : ' · partial')}</span>}</>}
      </button>;
    })}
  </div>;
}
