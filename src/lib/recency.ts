import type { NodeInfo } from '../contracts';

const day = new Intl.DateTimeFormat(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
const exact = new Intl.DateTimeFormat(undefined, { dateStyle: 'long', timeStyle: 'short' });

/** When anything in a folder last changed, or when a file was modified. */
export function lastChange(node: NodeInfo): { text: string; title: string } {
  const value = node.kind === 'directory' ? node.newestModified : node.modified;
  const time = value ? new Date(value) : null;
  if (!time || Number.isNaN(time.getTime())) {
    return {
      text: '—',
      title: node.kind === 'directory' ? 'Not recorded yet. Scans saved by earlier versions lack it; scan again to see when this folder last changed.' : 'Modification time unavailable',
    };
  }
  return {
    text: day.format(time),
    title: `${node.kind === 'directory' ? 'Latest change inside this folder' : 'Modified'}: ${exact.format(time)}`,
  };
}
