import { useEffect, useRef, type ReactNode } from 'react';
import { X } from 'lucide-react';
export const native = '__TAURI_INTERNALS__' in window;
export const errorText = (error: unknown) => typeof error === 'string' ? error : error instanceof Error ? error.message : 'Something went wrong. Please try again.';
export default function Modal({ title, children, onClose, className = '' }: { title: string; children: ReactNode; onClose: () => void; className?: string }) {
  const panel = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  useEffect(() => {
    const oldFocus = document.activeElement as HTMLElement | null;
    const focusable = () => panel.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), a[href], [tabindex="0"]');
    focusable()?.[0]?.focus();
    const handleKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); closeRef.current(); }
      if (event.key === 'Tab') {
        const elements = focusable();
        if (!elements?.length) return;
        const first = elements[0], last = elements[elements.length - 1];
        if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
      }
    };
    document.addEventListener('keydown', handleKey);
    return () => { document.removeEventListener('keydown', handleKey); oldFocus?.focus(); };
  }, []);
  return <div className="modal-backdrop"><div ref={panel} className={`modal ${className}`} role="dialog" aria-modal="true" aria-label={title}><div className="modal-heading"><div><span className="eyebrow">SPACETRACE</span><h2>{title}</h2></div><button className="icon-button" onClick={onClose} aria-label="Close dialog"><X size={20} /></button></div>{children}</div></div>;
}
