import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Info, Loader2, ShieldCheck, Trash2, TriangleAlert } from 'lucide-react';
import type { DeleteOutcome, ScanInfo } from '../contracts';
import Modal, { native, errorText } from './Modal';

export default function DeleteScanDialog({ scan, onClose, onDeleted }: { scan: ScanInfo; onClose: () => void; onDeleted: (outcome: DeleteOutcome) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const remove = async () => {
    setBusy(true); setError('');
    try { onDeleted(await invoke<DeleteOutcome>('delete_scan', { scanId: scan.id })); }
    catch (error) { setError(errorText(error)); setBusy(false); }
  };
  const started = new Date(scan.startedAt).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
  return <Modal title="Delete this saved scan?" onClose={busy ? () => {} : onClose} className="confirm-modal">
    <p className="modal-intro">SpaceTrace removes its saved results for <strong>{scan.rootName || scan.source.root}</strong> from {started}. Nothing on the scanned location changes: no files or folders are deleted.</p>
    <div className="notice"><Info size={17} /><span>Export the report first if you want to keep it. For large scans, freeing the space can take a minute.</span></div>
    {error && <div className="error-notice" role="alert"><TriangleAlert size={17} /><span>{error}</span></div>}
    <div className="modal-footer"><span><ShieldCheck size={14} />Only SpaceTrace’s own records are removed</span><div className="footer-buttons"><button className="button secondary" onClick={onClose} disabled={busy}>Keep scan</button><button className="button danger" onClick={remove} disabled={busy || !native}>{busy ? <Loader2 size={16} className="spin" /> : <Trash2 size={16} />}{busy ? 'Deleting…' : 'Delete saved scan'}</button></div></div>
  </Modal>;
}
