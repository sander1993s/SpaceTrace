export interface WeightedItem<T> { value: number; data: T }
export interface Tile<T> { x: number; y: number; width: number; height: number; data: T; value: number }

/** Squarifies positive weights, preserving area even for very uneven folders. */
export function layoutTreemap<T>(items: WeightedItem<T>[], width: number, height: number): Tile<T>[] {
  if (!(width > 0 && height > 0)) return [];
  const positive = items.filter(item => Number.isFinite(item.value) && item.value > 0).sort((a, b) => b.value - a.value);
  const total = positive.reduce((sum, item) => sum + item.value, 0);
  if (!total) return [];
  const scale = width * height / total;
  const pending = positive.map(item => ({ ...item, area: item.value * scale }));
  const result: Tile<T>[] = [];
  let x = 0, y = 0, w = width, h = height, index = 0;
  const worst = (row: typeof pending, side: number) => {
    if (!row.length || !side) return Infinity;
    const sum = row.reduce((value, item) => value + item.area, 0);
    return Math.max(side * side * row[0].area / (sum * sum), sum * sum / (side * side * row[row.length - 1].area));
  };
  while (index < pending.length) {
    const row = [pending[index++]];
    const side = Math.min(w, h);
    while (index < pending.length && worst([...row, pending[index]], side) <= worst(row, side)) row.push(pending[index++]);
    const sum = row.reduce((value, item) => value + item.area, 0);
    if (w >= h) {
      const rowWidth = h > 0 ? sum / h : 0;
      let offset = y;
      for (const item of row) {
        const itemHeight = rowWidth > 0 ? item.area / rowWidth : 0;
        result.push({ x, y: offset, width: rowWidth, height: itemHeight, value: item.value, data: item.data });
        offset += itemHeight;
      }
      x += rowWidth; w = Math.max(0, w - rowWidth);
    } else {
      const rowHeight = w > 0 ? sum / w : 0;
      let offset = x;
      for (const item of row) {
        const itemWidth = rowHeight > 0 ? item.area / rowHeight : 0;
        result.push({ x: offset, y, width: itemWidth, height: rowHeight, value: item.value, data: item.data });
        offset += itemWidth;
      }
      y += rowHeight; h = Math.max(0, h - rowHeight);
    }
  }
  return result;
}

export function byteValue(value: string | null | undefined): number {
  if (value == null) return 0;
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 0;
}

export function formatBytes(value: string | number | null | undefined): string {
  if (value == null) return 'Unavailable';
  const bytes = typeof value === 'number' ? value : byteValue(value);
  if (!bytes) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toLocaleString(undefined, { maximumFractionDigits: index === 0 ? 0 : 1 })} ${units[index]}`;
}

export const tileColors = ['#347fe8', '#1b9fbb', '#6874d6', '#419c92', '#9572c9', '#547ca6', '#4671bc', '#6391c5'];
export function nodeColor(name: string): string {
  let hash = 0;
  for (let i = 0; i < name.length; i++) hash = (hash * 31 + name.charCodeAt(i)) >>> 0;
  return tileColors[hash % tileColors.length];
}
