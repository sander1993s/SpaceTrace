import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import { Download, FileArchive, FileJson, Info, Loader2, Rows3, TriangleAlert } from 'lucide-react';
import type { ScanInfo } from '../contracts';
import Modal, { native, errorText } from './Modal';

type Layout = 'json' | 'ndjson';
/** Above this size a single JSON document becomes hard to open or stream. */
const LARGE_SCAN_ENTRIES = 250_000;

export default function ExportDialog({ scan, onClose, onExported }: { scan: ScanInfo; onClose: () => void; onExported: (message: string) => void }) {
  const large = scan.entries >= LARGE_SCAN_ENTRIES;
  const [layout, setLayout] = useState<Layout>(large ? 'ndjson' : 'json');
  const [compress, setCompress] = useState(large);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const extension = compress ? `${layout}.gz` : layout;
  const run = async () => {
    setBusy(true); setError('');
    try {
      const path = await save({
        title: 'Export scan report',
        defaultPath: `SpaceTrace-${scan.rootName.replace(/[^a-zA-Z0-9_-]/g, '-')}-${scan.startedAt.slice(0, 10)}.${extension}`,
        filters: [{ name: compress ? 'Compressed report' : layout === 'json' ? 'JSON report' : 'NDJSON report', extensions: [compress ? 'gz' : layout] }],
      });
      if (!path) { setBusy(false); return; }
      await invoke('export_scan', { scanId: scan.id, path });
      onExported('Your scan report has been exported.');
    } catch (error) { setError(errorText(error)); setBusy(false); }
  };
  const options = [
    { value: 'json', Icon: FileJson, title: 'JSON document', text: 'One document that most tools open directly. Best for small and medium scans.' },
    { value: 'ndjson', Icon: Rows3, title: 'NDJSON, one record per line', text: 'A header, every item, every issue and an end marker, each on its own line. Tools can read it line by line, so it suits large scans.' },
  ] as const;
  return <Modal title="Export a scan report" onClose={busy ? () => {} : onClose} className="export-modal">
    <p className="modal-intro">Reports contain folder names, paths and sizes, never file contents or SSH credentials.</p>
    <div className="export-options" role="radiogroup" aria-label="Report format">{options.map(({ value, Icon, title, text }) => <label key={value} className={`export-option ${layout === value ? 'active' : ''}`}><input type="radio" name="report-layout" value={value} checked={layout === value} onChange={() => setLayout(value)} disabled={busy} /><Icon size={19} /><span><strong>{title}</strong><small>{text}</small></span></label>)}</div>
    <label className={`export-option compact ${compress ? 'active' : ''}`}><input type="checkbox" checked={compress} onChange={event => setCompress(event.target.checked)} disabled={busy} /><FileArchive size={19} /><span><strong>Compress with gzip</strong><small>Reports often shrink more than tenfold. Most tools read .gz files directly.</small></span></label>
    {large && <div className="notice"><Info size={17} /><span>This scan has {scan.entries.toLocaleString()} entries, so NDJSON with gzip is selected to keep the report manageable.</span></div>}
    {error && <div className="error-notice" role="alert"><TriangleAlert size={17} /><span>{error}</span></div>}
    <div className="modal-footer"><span>Saves as <code>.{extension}</code></span><button className="button primary" onClick={run} disabled={busy || !native}>{busy ? <Loader2 size={17} className="spin" /> : <Download size={17} />}{busy ? 'Exporting…' : 'Choose location and export'}</button></div>
  </Modal>;
}
