import type { NodeInfo } from '../contracts';
import { byteValue } from './treemap';

export interface BranchData {
  nodeId: number;
  node: NodeInfo | null;
  children: NodeInfo[];
  totalChildren: number;
  requestedCount: number;
  loading: boolean;
  error: string | null;
  version: number;
  revision: number;
}

export interface VisibleNodeItem {
  type: 'node';
  node: NodeInfo;
  parent: NodeInfo | null;
  depth: number;
  expanded: boolean;
}

export interface VisiblePaginationItem {
  type: 'pagination';
  parentNode: NodeInfo;
  depth: number;
  loadedCount: number;
  totalCount: number;
  loading: boolean;
}

export interface VisibleLoadingItem {
  type: 'loading';
  parentNode: NodeInfo;
  depth: number;
}

export interface VisibleEmptyItem {
  type: 'empty';
  parentNode: NodeInfo;
  depth: number;
}

export interface VisibleErrorItem {
  type: 'error';
  parentNode: NodeInfo;
  depth: number;
  error: string;
}

export type TreeItem =
  | VisibleNodeItem
  | VisiblePaginationItem
  | VisibleLoadingItem
  | VisibleEmptyItem
  | VisibleErrorItem;

export function deduplicateNodes(nodes: NodeInfo[]): NodeInfo[] {
  const seen = new Set<number>();
  const result: NodeInfo[] = [];
  for (const node of nodes) {
    if (!seen.has(node.id)) {
      seen.add(node.id);
      result.push(node);
    }
  }
  return result;
}

export function calculateShare(
  childSizeStr: string | null | undefined,
  parentSizeStr: string | null | undefined
): { sharePercent: number; shareText: string; isAvailable: boolean } {
  if (childSizeStr == null || parentSizeStr == null) {
    return { sharePercent: 0, shareText: '—', isAvailable: false };
  }
  const childVal = byteValue(childSizeStr);
  const parentVal = byteValue(parentSizeStr);
  if (parentVal <= 0) {
    return { sharePercent: 0, shareText: '—', isAvailable: false };
  }
  const share = (childVal / parentVal) * 100;
  const shareText = share < 0.1 && share > 0 ? '<0.1%' : `${share.toFixed(1)}%`;
  return { sharePercent: Math.min(share, 100), shareText, isAvailable: true };
}

export function flattenTree(
  rootId: number,
  branches: Map<number, BranchData>,
  expandedIds: Set<number>
): { items: TreeItem[]; maxDepth: number } {
  const items: TreeItem[] = [];
  let maxDepth = 0;

  const rootBranch = branches.get(rootId);
  if (!rootBranch || (!rootBranch.node && rootBranch.loading)) {
    return { items, maxDepth: 0 };
  }

  const rootNode = rootBranch.node;
  if (!rootNode) return { items, maxDepth: 0 };

  const isRootExpanded = expandedIds.has(rootId);
  items.push({
    type: 'node',
    node: rootNode,
    parent: null,
    depth: 0,
    expanded: isRootExpanded
  });

  if (!isRootExpanded) {
    return { items, maxDepth: 0 };
  }

  type Frame = {
    parentNode: NodeInfo;
    depth: number;
    branch: BranchData;
    childIdx: number;
  };

  const stack: Frame[] = [
    {
      parentNode: rootNode,
      depth: 1,
      branch: rootBranch,
      childIdx: 0
    }
  ];

  while (stack.length > 0) {
    const frame = stack[stack.length - 1];
    const { parentNode, depth, branch } = frame;

    if (depth > maxDepth) maxDepth = depth;

    if (branch.loading && branch.children.length === 0) {
      items.push({ type: 'loading', parentNode, depth });
      stack.pop();
      continue;
    }

    if (branch.error && branch.children.length === 0) {
      items.push({ type: 'error', parentNode, depth, error: branch.error });
      stack.pop();
      continue;
    }

    if (!branch.loading && branch.totalChildren === 0) {
      items.push({ type: 'empty', parentNode, depth });
      stack.pop();
      continue;
    }

    if (frame.childIdx < branch.children.length) {
      const child = branch.children[frame.childIdx];
      frame.childIdx++;

      const isDir = child.kind === 'directory';
      const isExpanded = isDir && expandedIds.has(child.id);

      let resolvedChild = child;
      const childBranch = isDir ? branches.get(child.id) : undefined;
      if (
        childBranch?.node &&
        (childBranch.revision ?? 0) > (branch.revision ?? 0)
      ) {
        resolvedChild = childBranch.node;
      }

      items.push({
        type: 'node',
        node: resolvedChild,
        parent: parentNode,
        depth,
        expanded: isExpanded
      });

      if (isExpanded) {
        if (childBranch) {
          stack.push({
            parentNode: resolvedChild,
            depth: depth + 1,
            branch: childBranch,
            childIdx: 0
          });
        } else {
          items.push({ type: 'loading', parentNode: resolvedChild, depth: depth + 1 });
        }
      }
    } else {
      if (branch.children.length < branch.totalChildren && !branch.error) {
        items.push({
          type: 'pagination',
          parentNode,
          depth,
          loadedCount: branch.children.length,
          totalCount: branch.totalChildren,
          loading: branch.loading
        });
      }

      if (branch.error && branch.children.length > 0) {
        items.push({
          type: 'error',
          parentNode,
          depth,
          error: branch.error
        });
      }

      stack.pop();
    }
  }

  return { items, maxDepth };
}

export function getVisibleExpandedBranchIds(
  rootId: number,
  branches: Map<number, BranchData>,
  expandedIds: Set<number>
): number[] {
  const result: number[] = [rootId];
  if (!expandedIds.has(rootId)) return result;

  const visited = new Set<number>();
  const queue = [rootId];
  visited.add(rootId);

  while (queue.length > 0) {
    const id = queue.shift()!;
    const branch = branches.get(id);
    if (branch) {
      for (const child of branch.children) {
        if (child.kind === 'directory' && expandedIds.has(child.id) && !visited.has(child.id)) {
          visited.add(child.id);
          result.push(child.id);
          queue.push(child.id);
        }
      }
    }
  }
  return result;
}
