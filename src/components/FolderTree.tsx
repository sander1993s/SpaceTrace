import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  ArrowDown,
  ChevronDown,
  ChevronRight,
  ChevronsDownUp,
  File,
  Folder,
  FolderOpen,
  Loader2,
  RotateCw,
  TriangleAlert
} from 'lucide-react';
import type { BrowseResult, Metric, NodeInfo } from '../contracts';
import { formatBytes, nodeColor } from '../lib/treemap';
import { native, errorText } from './Modal';
import {
  type BranchData,
  calculateShare,
  deduplicateNodes,
  flattenTree,
  getVisibleExpandedBranchIds,
  type VisibleNodeItem
} from '../lib/folderTree';

interface FolderTreeProps {
  scanId: string;
  scanStatus: 'scanning' | 'completed' | 'cancelled' | 'failed';
  metric: Metric;
  active: boolean;
  onFileSelect: (node: NodeInfo) => void;
}

function getAncestorChain(targetId: number, branches: Map<number, BranchData>): number[] {
  const ancestors: number[] = [];
  let curr = targetId;
  const visited = new Set<number>();
  while (curr !== 0 && !visited.has(curr)) {
    visited.add(curr);
    let parentId: number | null = null;
    for (const [bId, b] of branches.entries()) {
      if (b.children.some(c => c.id === curr)) {
        parentId = bId;
        break;
      }
    }
    if (parentId === null) {
      ancestors.push(0);
      break;
    }
    ancestors.push(parentId);
    curr = parentId;
  }
  if (!ancestors.includes(0)) {
    ancestors.push(0);
  }
  return ancestors;
}

export default function FolderTree({
  scanId,
  scanStatus,
  metric,
  active,
  onFileSelect
}: FolderTreeProps) {
  const [expandedIds, setExpandedIds] = useState<Set<number>>(() => new Set<number>([0]));
  const [branches, setBranches] = useState<Map<number, BranchData>>(() => new Map<number, BranchData>());
  const [focusedNodeId, setFocusedNodeId] = useState<number>(0);
  const focusedNodeIdRef = useRef<number>(0);
  focusedNodeIdRef.current = focusedNodeId;

  const branchesRef = useRef<Map<number, BranchData>>(branches);
  const expandedIdsRef = useRef<Set<number>>(expandedIds);
  const activeRef = useRef<boolean>(active);
  activeRef.current = active;
  const metricRef = useRef<Metric>(metric);
  metricRef.current = metric;
  const scanStatusRef = useRef<string>(scanStatus);
  scanStatusRef.current = scanStatus;

  const contextGenRef = useRef<number>(0);
  const isMountedRef = useRef<boolean>(true);
  const isPollingBatchRef = useRef<boolean>(false);
  const finalRefreshPendingRef = useRef<boolean>(false);
  const metricDirtyRef = useRef<boolean>(false);

  const inFlightJobs = useRef<Map<number, number>>(new Map());
  const nextAttemptTokenRef = useRef<number>(0);
  const nextSnapshotSeqRef = useRef<number>(0);
  const jobQueue = useRef<Array<{ nodeId: number; targetCount: number; isRefresh: boolean }>>([]);
  const focusRecoveryRef = useRef<{ focusedId: number; ancestors: number[]; shouldRecover: boolean } | null>(null);

  const rowRefs = useRef<Map<number, HTMLElement>>(new Map());
  const treeRef = useRef<HTMLDivElement | null>(null);

  const prevActiveRef = useRef<boolean>(active);
  const prevStatusRef = useRef<string>(scanStatus);
  const prevMetricRef = useRef<Metric>(metric);

  const pumpQueueRef = useRef<() => void>(() => {});
  const refreshVisibleBranchesRef = useRef<() => void>(() => {});
  const ensureRootScheduledRef = useRef<() => void>(() => {});

  const commitBranches = useCallback(
    (updater: (prev: Map<number, BranchData>) => Map<number, BranchData>) => {
      const next = updater(branchesRef.current);
      branchesRef.current = next;
      setBranches(next);
      return next;
    },
    []
  );

  const commitExpandedIds = useCallback(
    (updater: (prev: Set<number>) => Set<number>) => {
      const next = updater(expandedIdsRef.current);
      expandedIdsRef.current = next;
      setExpandedIds(next);
      return next;
    },
    []
  );

  const isJobEligible = useCallback((nodeId: number): boolean => {
    if (nodeId === 0) return true;
    const visibleIds = getVisibleExpandedBranchIds(0, branchesRef.current, expandedIdsRef.current);
    return visibleIds.includes(nodeId);
  }, []);

  const queueJob = useCallback((nodeId: number, targetCount: number, isRefresh: boolean) => {
    const existingIdx = jobQueue.current.findIndex(j => j.nodeId === nodeId);
    if (existingIdx !== -1) {
      const existing = jobQueue.current[existingIdx];
      existing.targetCount = Math.max(existing.targetCount, targetCount);
      existing.isRefresh = existing.isRefresh || isRefresh;
    } else {
      jobQueue.current.push({ nodeId, targetCount, isRefresh });
    }
  }, []);

  const runJob = useCallback(
    async (nodeId: number, targetCount: number, isRefresh: boolean) => {
      const attemptToken = ++nextAttemptTokenRef.current;
      inFlightJobs.current.set(nodeId, attemptToken);

      try {
        if (!native || !isMountedRef.current || !activeRef.current) return;
        if (!isJobEligible(nodeId)) return;

        const currentContextGen = contextGenRef.current;
        const currentMetric = metricRef.current;
        const existingBranch = branchesRef.current.get(nodeId);
        const currentBranchVersion = existingBranch?.version ?? 1;
        const requestSeq = ++nextSnapshotSeqRef.current;

        if (!isRefresh) {
          commitBranches(prev => {
            const next = new Map(prev);
            const b = next.get(nodeId);
            next.set(nodeId, {
              nodeId,
              node: b?.node ?? null,
              children: b?.children ?? [],
              totalChildren: b?.totalChildren ?? 0,
              requestedCount: Math.max(targetCount, b?.requestedCount || 0),
              loading: true,
              error: null,
              version: currentBranchVersion,
              revision: b?.revision ?? 0
            });
            return next;
          });
        }

        let offset = 0;
        let firstResult: BrowseResult | null = null;
        const allChildren: NodeInfo[] = [];
        let errorMsg: string | null = null;
        const PAGE_SIZE = 200;

        while (offset < targetCount) {
          if (
            !isMountedRef.current ||
            !activeRef.current ||
            contextGenRef.current !== currentContextGen ||
            metricRef.current !== currentMetric ||
            branchesRef.current.get(nodeId)?.version !== currentBranchVersion ||
            !isJobEligible(nodeId)
          ) {
            return;
          }

          const limit = Math.min(PAGE_SIZE, targetCount - offset);
          let res: BrowseResult;
          try {
            res = await invoke<BrowseResult>('browse', {
              scanId,
              nodeId,
              search: '',
              offset,
              limit,
              metric: currentMetric
            });
          } catch (err) {
            errorMsg = errorText(err);
            break;
          }

          if (
            !isMountedRef.current ||
            !activeRef.current ||
            contextGenRef.current !== currentContextGen ||
            metricRef.current !== currentMetric ||
            branchesRef.current.get(nodeId)?.version !== currentBranchVersion ||
            !isJobEligible(nodeId)
          ) {
            return;
          }

          if (!firstResult) {
            firstResult = res;
          }
          allChildren.push(...res.children);
          offset += res.children.length;

          if (res.children.length < limit || allChildren.length >= res.totalChildren) {
            break;
          }
        }

        if (
          !isMountedRef.current ||
          !activeRef.current ||
          contextGenRef.current !== currentContextGen ||
          metricRef.current !== currentMetric ||
          branchesRef.current.get(nodeId)?.version !== currentBranchVersion ||
          !isJobEligible(nodeId)
        ) {
          return;
        }

        if (errorMsg) {
          commitBranches(prev => {
            const next = new Map(prev);
            const b = next.get(nodeId);
            if (b && b.version === currentBranchVersion) {
              next.set(nodeId, {
                ...b,
                loading: false,
                error: errorMsg
              });
            }
            return next;
          });
        } else {
          const deduped = deduplicateNodes(allChildren);

          if (treeRef.current?.contains(document.activeElement)) {
            let currentFocus = focusedNodeIdRef.current;
            for (const [id, el] of rowRefs.current.entries()) {
              if (el === document.activeElement || el.contains(document.activeElement)) {
                currentFocus = id;
                break;
              }
            }
            const ancestors = getAncestorChain(currentFocus, branchesRef.current);
            focusRecoveryRef.current = {
              focusedId: currentFocus,
              ancestors,
              shouldRecover: true
            };
          } else {
            focusRecoveryRef.current = null;
          }

          commitBranches(prev => {
            const next = new Map(prev);
            const b = next.get(nodeId);
            if (b && b.version === currentBranchVersion) {
              const keepExistingNode = b.node && (b.revision ?? 0) > requestSeq;
              const resolvedNode = keepExistingNode ? b.node : (firstResult?.node ?? b.node ?? null);
              const resolvedRev = keepExistingNode ? b.revision : requestSeq;

              next.set(nodeId, {
                nodeId,
                node: resolvedNode,
                children: deduped,
                totalChildren: firstResult?.totalChildren ?? deduped.length,
                requestedCount: Math.max(targetCount, deduped.length, b.requestedCount || 0),
                loading: false,
                error: null,
                version: currentBranchVersion,
                revision: resolvedRev
              });
            }
            return next;
          });
        }
      } finally {
        if (inFlightJobs.current.get(nodeId) === attemptToken) {
          inFlightJobs.current.delete(nodeId);
        }
        queueMicrotask(() => {
          pumpQueueRef.current();
        });
      }
    },
    [scanId, isJobEligible, commitBranches]
  );

  const pumpQueue = useCallback(() => {
    if (!isMountedRef.current || !activeRef.current) return;

    jobQueue.current = jobQueue.current.filter(j => isJobEligible(j.nodeId));

    while (inFlightJobs.current.size < 2 && jobQueue.current.length > 0) {
      const nextIdx = jobQueue.current.findIndex(j => !inFlightJobs.current.has(j.nodeId));
      if (nextIdx === -1) break;

      const [job] = jobQueue.current.splice(nextIdx, 1);
      void runJob(job.nodeId, job.targetCount, job.isRefresh);
    }

    if (inFlightJobs.current.size === 0 && jobQueue.current.length === 0) {
      isPollingBatchRef.current = false;
      if (finalRefreshPendingRef.current && activeRef.current) {
        finalRefreshPendingRef.current = false;
        refreshVisibleBranchesRef.current();
      }
    }
  }, [isJobEligible, runJob]);

  pumpQueueRef.current = pumpQueue;

  const refreshVisibleBranches = useCallback(() => {
    if (!native || !activeRef.current) return;
    const visibleIds = getVisibleExpandedBranchIds(0, branchesRef.current, expandedIdsRef.current);
    if (!visibleIds.length) {
      const rootBranch = branchesRef.current.get(0);
      if (!rootBranch?.node && (!rootBranch?.error || scanStatusRef.current === 'scanning')) {
        queueJob(0, rootBranch?.requestedCount || 200, false);
        pumpQueue();
      }
      return;
    }

    isPollingBatchRef.current = true;
    for (const id of visibleIds) {
      const b = branchesRef.current.get(id);
      if (id === 0 && b?.error && !b?.node && scanStatusRef.current !== 'scanning') {
        continue;
      }
      const count = Math.max(b?.requestedCount || 200, b?.children.length || 0);
      queueJob(id, count, true);
    }
    pumpQueue();
  }, [queueJob, pumpQueue]);

  refreshVisibleBranchesRef.current = refreshVisibleBranches;

  const ensureRootScheduled = useCallback(() => {
    if (!native) return;
    const root = branchesRef.current.get(0);
    if (!root) {
      commitBranches(prev => {
        const next = new Map(prev);
        next.set(0, {
          nodeId: 0,
          node: null,
          children: [],
          totalChildren: 0,
          requestedCount: 200,
          loading: true,
          error: null,
          version: 1,
          revision: 0
        });
        return next;
      });
      queueJob(0, 200, false);
    } else if (!root.node && !root.error) {
      if (!root.loading) {
        commitBranches(prev => {
          const next = new Map(prev);
          const r = next.get(0);
          if (r) next.set(0, { ...r, loading: true });
          return next;
        });
      }
      queueJob(0, root.requestedCount || 200, false);
    }
  }, [commitBranches, queueJob]);

  ensureRootScheduledRef.current = ensureRootScheduled;

  useEffect(() => {
    isMountedRef.current = true;
    ensureRootScheduledRef.current();
    if (activeRef.current) {
      pumpQueueRef.current();
    }
    return () => {
      isMountedRef.current = false;
      contextGenRef.current++;
      jobQueue.current = [];
    };
  }, []);

  useEffect(() => {
    if (prevMetricRef.current === metric) return;
    prevMetricRef.current = metric;
    metricRef.current = metric;

    contextGenRef.current++;
    jobQueue.current = [];
    isPollingBatchRef.current = false;

    commitBranches(prev => {
      const next = new Map(prev);
      let changed = false;
      for (const [id, b] of next.entries()) {
        if (b.loading) {
          next.set(id, { ...b, loading: false, version: b.version + 1 });
          changed = true;
        }
      }
      return changed ? next : prev;
    });

    if (activeRef.current) {
      refreshVisibleBranchesRef.current();
    } else {
      metricDirtyRef.current = true;
    }
  }, [metric, commitBranches]);

  useEffect(() => {
    const prevStatus = prevStatusRef.current;
    prevStatusRef.current = scanStatus;
    scanStatusRef.current = scanStatus;

    if (prevStatus === 'scanning' && scanStatus !== 'scanning') {
      finalRefreshPendingRef.current = true;
      if (
        activeRef.current &&
        inFlightJobs.current.size === 0 &&
        jobQueue.current.length === 0 &&
        !isPollingBatchRef.current
      ) {
        finalRefreshPendingRef.current = false;
        refreshVisibleBranchesRef.current();
      }
    }
  }, [scanStatus]);

  useEffect(() => {
    const prevActive = prevActiveRef.current;
    prevActiveRef.current = active;
    activeRef.current = active;

    if (!prevActive && active) {
      ensureRootScheduledRef.current();
      metricDirtyRef.current = false;
      finalRefreshPendingRef.current = false;

      const visibleIds = getVisibleExpandedBranchIds(0, branchesRef.current, expandedIdsRef.current);
      for (const id of visibleIds) {
        const b = branchesRef.current.get(id);
        if (id === 0 && b?.error && !b?.node && scanStatusRef.current !== 'scanning') {
          continue;
        }
        const hasLoaded = (b?.children.length ?? 0) > 0 || !!b?.node;
        const count = Math.max(b?.requestedCount || 200, b?.children.length || 0);
        queueJob(id, count, hasLoaded);
      }
      pumpQueueRef.current();
    } else if (prevActive && !active) {
      contextGenRef.current++;
      jobQueue.current = [];
      isPollingBatchRef.current = false;

      commitBranches(prev => {
        const next = new Map(prev);
        let changed = false;
        for (const [id, b] of next.entries()) {
          if (b.loading) {
            next.set(id, { ...b, loading: false, version: b.version + 1 });
            changed = true;
          }
        }
        return changed ? next : prev;
      });
    }
  }, [active, commitBranches, queueJob]);

  useEffect(() => {
    if (!native || !active || scanStatus !== 'scanning') return;

    const interval = setInterval(() => {
      if (!activeRef.current || scanStatusRef.current !== 'scanning') return;
      if (isPollingBatchRef.current) return;
      if (inFlightJobs.current.size > 0 || jobQueue.current.length > 0) return;

      const visibleIds = getVisibleExpandedBranchIds(0, branchesRef.current, expandedIdsRef.current);
      if (!visibleIds.length) return;

      isPollingBatchRef.current = true;
      for (const id of visibleIds) {
        const b = branchesRef.current.get(id);
        if (id === 0 && b?.error && !b?.node && scanStatusRef.current !== 'scanning') {
          continue;
        }
        const count = Math.max(b?.requestedCount || 200, b?.children.length || 0);
        queueJob(id, count, true);
      }
      pumpQueueRef.current();
    }, 1200);

    return () => clearInterval(interval);
  }, [active, scanStatus, queueJob]);

  const toggleExpand = useCallback(
    (node: NodeInfo) => {
      if (node.kind !== 'directory') return;

      if (expandedIdsRef.current.has(node.id)) {
        const nextExpanded = new Set(expandedIdsRef.current);
        nextExpanded.delete(node.id);

        const descendantIds: number[] = [];
        const visited = new Set<number>([node.id]);
        const stack: number[] = [node.id];

        while (stack.length > 0) {
          const currId = stack.pop()!;
          const b = branchesRef.current.get(currId);
          if (b) {
            for (const c of b.children) {
              if (c.kind === 'directory' && !visited.has(c.id)) {
                visited.add(c.id);
                descendantIds.push(c.id);
                stack.push(c.id);
              }
            }
          }
        }

        for (const dId of descendantIds) {
          nextExpanded.delete(dId);
        }

        commitBranches(prev => {
          const next = new Map(prev);
          const toUpdate = [node.id, ...descendantIds];
          for (const id of toUpdate) {
            const b = next.get(id);
            if (b) {
              next.set(id, {
                ...b,
                version: b.version + 1,
                loading: false
              });
            }
          }
          return next;
        });

        const cancelledIds = new Set([node.id, ...descendantIds]);
        jobQueue.current = jobQueue.current.filter(j => !cancelledIds.has(j.nodeId));

        commitExpandedIds(() => nextExpanded);

        const currentFocus = focusedNodeIdRef.current;
        if (currentFocus === node.id || descendantIds.includes(currentFocus)) {
          focusedNodeIdRef.current = node.id;
          setFocusedNodeId(node.id);
          if (treeRef.current?.contains(document.activeElement)) {
            rowRefs.current.get(node.id)?.focus();
          }
        }
      } else {
        const nextExpanded = new Set(expandedIdsRef.current);
        nextExpanded.add(node.id);
        commitExpandedIds(() => nextExpanded);

        const b = branchesRef.current.get(node.id);
        const newVersion = (b?.version ?? 0) + 1;
        const requestedCount = b?.requestedCount || 200;

        commitBranches(prev => {
          const next = new Map(prev);
          next.set(node.id, {
            nodeId: node.id,
            node: b?.node ?? node,
            children: b?.children ?? [],
            totalChildren: b?.totalChildren ?? 0,
            requestedCount,
            loading: !b || b.children.length === 0,
            error: null,
            version: newVersion,
            revision: b?.revision ?? 0
          });
          return next;
        });

        queueJob(node.id, requestedCount, b ? b.children.length > 0 : false);
        pumpQueue();
      }
    },
    [commitExpandedIds, commitBranches, queueJob, pumpQueue]
  );

  const handleLoadMore = useCallback(
    (parentNode: NodeInfo) => {
      const b = branchesRef.current.get(parentNode.id);
      if (!b || b.loading) return;

      const nextCount = Math.max(b.requestedCount || 0, b.children.length + 200);
      const newVersion = b.version + 1;

      commitBranches(prev => {
        const next = new Map(prev);
        const existing = next.get(parentNode.id);
        if (existing) {
          next.set(parentNode.id, {
            ...existing,
            loading: true,
            requestedCount: nextCount,
            version: newVersion
          });
        }
        return next;
      });

      queueJob(parentNode.id, nextCount, false);
      pumpQueue();
    },
    [commitBranches, queueJob, pumpQueue]
  );

  const handleRetry = useCallback(
    (nodeId: number) => {
      const b = branchesRef.current.get(nodeId);
      const count = b?.requestedCount || 200;
      const newVersion = (b?.version ?? 0) + 1;

      commitBranches(prev => {
        const next = new Map(prev);
        const existing = next.get(nodeId);
        if (existing) {
          next.set(nodeId, {
            ...existing,
            loading: true,
            error: null,
            version: newVersion
          });
        } else if (nodeId === 0) {
          next.set(0, {
            nodeId: 0,
            node: null,
            children: [],
            totalChildren: 0,
            requestedCount: count,
            loading: true,
            error: null,
            version: newVersion,
            revision: 0
          });
        }
        return next;
      });

      queueJob(nodeId, count, false);
      pumpQueue();
    },
    [commitBranches, queueJob, pumpQueue]
  );

  const handleCollapseAll = useCallback(() => {
    commitExpandedIds(() => new Set([0]));
    commitBranches(prev => {
      const next = new Map(prev);
      for (const [id, b] of next.entries()) {
        if (id !== 0) {
          next.set(id, {
            ...b,
            version: b.version + 1,
            loading: false
          });
        }
      }
      return next;
    });
    jobQueue.current = jobQueue.current.filter(j => j.nodeId === 0);
    focusedNodeIdRef.current = 0;
    setFocusedNodeId(0);
    if (treeRef.current?.contains(document.activeElement)) {
      rowRefs.current.get(0)?.focus();
    }
  }, [commitExpandedIds, commitBranches]);

  const { items: visibleItems, maxDepth } = useMemo(
    () => flattenTree(0, branches, expandedIds),
    [branches, expandedIds]
  );

  const visibleNodes = useMemo(
    () => visibleItems.filter((item): item is VisibleNodeItem => item.type === 'node'),
    [visibleItems]
  );

  const effectiveFocusedId = useMemo(() => {
    if (visibleNodes.some(n => n.node.id === focusedNodeId)) {
      return focusedNodeId;
    }
    return visibleNodes[0]?.node.id ?? 0;
  }, [visibleNodes, focusedNodeId]);

  useEffect(() => {
    if (!focusRecoveryRef.current?.shouldRecover) return;

    const { focusedId, ancestors } = focusRecoveryRef.current;
    focusRecoveryRef.current = null;

    const stillPresent = visibleNodes.some(n => n.node.id === focusedId);
    if (stillPresent) return;

    const activeEl = document.activeElement;
    const focusLostFromRemovedRow =
      !activeEl ||
      activeEl === document.body ||
      activeEl === document.documentElement ||
      activeEl === treeRef.current;

    if (focusLostFromRemovedRow) {
      const nearestAncestor = ancestors.find(aId => visibleNodes.some(n => n.node.id === aId)) ?? 0;
      focusedNodeIdRef.current = nearestAncestor;
      setFocusedNodeId(nearestAncestor);
      const targetEl = rowRefs.current.get(nearestAncestor);
      targetEl?.focus();
    }
  }, [visibleNodes]);

  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      const target = event.target as HTMLElement | null;
      if (target && target.closest('button, input, select, textarea, a')) {
        return;
      }

      const currentIndex = visibleNodes.findIndex(n => n.node.id === effectiveFocusedId);
      if (currentIndex === -1 && visibleNodes.length === 0) return;

      const current = visibleNodes[currentIndex];

      switch (event.key) {
        case 'ArrowDown': {
          event.preventDefault();
          if (currentIndex < visibleNodes.length - 1) {
            const next = visibleNodes[currentIndex + 1];
            focusedNodeIdRef.current = next.node.id;
            setFocusedNodeId(next.node.id);
            rowRefs.current.get(next.node.id)?.focus();
          }
          break;
        }

        case 'ArrowUp': {
          event.preventDefault();
          if (currentIndex > 0) {
            const prev = visibleNodes[currentIndex - 1];
            focusedNodeIdRef.current = prev.node.id;
            setFocusedNodeId(prev.node.id);
            rowRefs.current.get(prev.node.id)?.focus();
          }
          break;
        }

        case 'Home': {
          event.preventDefault();
          if (visibleNodes.length > 0) {
            const first = visibleNodes[0];
            focusedNodeIdRef.current = first.node.id;
            setFocusedNodeId(first.node.id);
            rowRefs.current.get(first.node.id)?.focus();
          }
          break;
        }

        case 'End': {
          event.preventDefault();
          if (visibleNodes.length > 0) {
            const last = visibleNodes[visibleNodes.length - 1];
            focusedNodeIdRef.current = last.node.id;
            setFocusedNodeId(last.node.id);
            rowRefs.current.get(last.node.id)?.focus();
          }
          break;
        }

        case 'ArrowRight': {
          event.preventDefault();
          if (!current) break;
          if (current.node.kind === 'directory') {
            if (!current.expanded) {
              toggleExpand(current.node);
            } else if (
              currentIndex < visibleNodes.length - 1 &&
              visibleNodes[currentIndex + 1].depth > current.depth
            ) {
              const firstChild = visibleNodes[currentIndex + 1];
              focusedNodeIdRef.current = firstChild.node.id;
              setFocusedNodeId(firstChild.node.id);
              rowRefs.current.get(firstChild.node.id)?.focus();
            }
          }
          break;
        }

        case 'ArrowLeft': {
          event.preventDefault();
          if (!current) break;
          if (current.node.kind === 'directory' && current.expanded) {
            toggleExpand(current.node);
          } else if (current.parent) {
            focusedNodeIdRef.current = current.parent.id;
            setFocusedNodeId(current.parent.id);
            rowRefs.current.get(current.parent.id)?.focus();
          }
          break;
        }

        case 'Enter':
        case ' ': {
          event.preventDefault();
          if (!current) break;
          if (current.node.kind === 'directory') {
            toggleExpand(current.node);
          } else {
            onFileSelect(current.node);
          }
          break;
        }
      }
    },
    [visibleNodes, effectiveFocusedId, toggleExpand, onFileSelect]
  );

  if (!active) {
    return null;
  }

  const rootBranch = branches.get(0);
  const isScanning = scanStatus === 'scanning';
  const nameMinPx = Math.max(280, 180 + (maxDepth + 1) * 22);
  const gridTemplateColumns = `minmax(${nameMinPx}px, 1.8fr) 110px 140px 85px 45px`;

  return (
    <div className="tree-panel">
      <div className="tree-top-bar">
        <button
          type="button"
          className="button secondary small collapse-all-btn"
          onClick={handleCollapseAll}
          title="Collapse all folders"
          aria-label="Collapse all folders"
        >
          <ChevronsDownUp size={14} />
          Collapse all
        </button>
      </div>

      <div className="tree-view-wrapper">
        <div
          role="tree"
          aria-label="Folder tree"
          className="tree-grid"
          tabIndex={-1}
          onKeyDown={handleKeyDown}
          ref={treeRef}
          style={{ minWidth: `${nameMinPx + 380}px` }}
        >
          <div className="tree-header" role="presentation" style={{ gridTemplateColumns }}>
            <span className="tree-header-cell">Name</span>
            <span className="tree-header-cell align-right">
              {metric === 'logical' ? 'File size' : 'On disk'}
              <ArrowDown size={11} style={{ display: 'inline', marginLeft: 3 }} />
            </span>
            <span className="tree-header-cell" style={{ paddingLeft: 10 }}>
              Of parent
            </span>
            <span className="tree-header-cell align-right">Files</span>
            <span className="tree-header-cell align-center" aria-label="Status" />
          </div>

          {(!rootBranch || (!rootBranch.node && rootBranch.loading)) && (
            <div className="loading-line">
              <Loader2 className="spin" size={18} />
              Loading folder tree…
            </div>
          )}

          {rootBranch?.error && !rootBranch.node && (
            <div className="error-notice" style={{ margin: '16px 20px' }}>
              <TriangleAlert size={16} />
              <span>{rootBranch.error}</span>
              <button
                type="button"
                className="button secondary small"
                onClick={() => handleRetry(0)}
              >
                <RotateCw size={12} />
                Retry
              </button>
            </div>
          )}

          {rootBranch?.node &&
            visibleItems.map(item => {
              if (item.type === 'node') {
                const node = item.node;
                const isDir = node.kind === 'directory';
                const sizeStr = metric === 'logical' ? node.logicalBytes : node.allocatedBytes;
                const parentSizeStr = item.parent
                  ? metric === 'logical'
                    ? item.parent.logicalBytes
                    : item.parent.allocatedBytes
                  : null;
                const shareInfo = calculateShare(sizeStr, parentSizeStr);
                const isFocused = effectiveFocusedId === node.id;

                return (
                  <div
                    key={`node-${node.id}`}
                    ref={el => {
                      if (el) rowRefs.current.set(node.id, el);
                      else rowRefs.current.delete(node.id);
                    }}
                    role="treeitem"
                    aria-label={node.name}
                    aria-level={item.depth + 1}
                    aria-expanded={isDir ? item.expanded : undefined}
                    aria-selected={isFocused}
                    tabIndex={isFocused ? 0 : -1}
                    onFocus={() => {
                      focusedNodeIdRef.current = node.id;
                      setFocusedNodeId(node.id);
                    }}
                    className={`tree-row ${isFocused ? 'is-selected' : ''}`}
                    style={{ gridTemplateColumns }}
                    onClick={() => {
                      focusedNodeIdRef.current = node.id;
                      setFocusedNodeId(node.id);
                      if (isDir) {
                        toggleExpand(node);
                      } else {
                        onFileSelect(node);
                      }
                    }}
                    title={node.path}
                  >
                    <div className="tree-name-col">
                      <span className="tree-indent-wrapper">
                        {Array.from({ length: item.depth }).map((_, idx) => (
                          <span key={idx} className="tree-indent-guide" />
                        ))}
                      </span>

                      {isDir ? (
                        <span
                          className="tree-chevron"
                          aria-hidden="true"
                          onClick={e => {
                            e.stopPropagation();
                            focusedNodeIdRef.current = node.id;
                            setFocusedNodeId(node.id);
                            toggleExpand(node);
                          }}
                        >
                          {item.expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                        </span>
                      ) : (
                        <span className="tree-chevron-spacer" />
                      )}

                      <span className="tree-icon" aria-hidden="true">
                        {isDir ? (
                          item.expanded ? (
                            <FolderOpen size={16} style={{ color: nodeColor(node.name) }} />
                          ) : (
                            <Folder size={16} style={{ color: nodeColor(node.name) }} />
                          )
                        ) : (
                          <File size={16} />
                        )}
                      </span>

                      <span className="tree-name-text">{node.name}</span>

                      {node.shared && (
                        <span
                          className="shared-label"
                          title="Shares its underlying data with another hard link"
                        >
                          linked
                        </span>
                      )}

                      {!node.complete && (
                        <span
                          className={isScanning ? 'counting-dot' : 'incomplete-dot'}
                          title={isScanning ? 'Still scanning' : 'Incomplete measurement'}
                          aria-label={isScanning ? 'Still scanning' : 'Incomplete measurement'}
                        />
                      )}
                    </div>

                    <div className="tree-size-col">
                      {sizeStr === null ? 'Unavailable' : formatBytes(sizeStr)}
                    </div>

                    <div className="tree-share-col">
                      <div className="share-cell">
                        <div>
                          <i
                            style={{
                              width: `${shareInfo.sharePercent}%`,
                              background: nodeColor(node.name)
                            }}
                          />
                        </div>
                        <span>{shareInfo.shareText}</span>
                      </div>
                    </div>

                    <div className="tree-files-col">
                      {isDir ? node.files.toLocaleString() : '—'}
                    </div>

                    <div className="tree-status-col" />
                  </div>
                );
              }

              if (item.type === 'pagination') {
                return (
                  <div
                    key={`pagination-${item.parentNode.id}`}
                    role="none"
                    className="tree-status-row"
                    style={{ paddingLeft: 16 + item.depth * 18 }}
                  >
                    <button
                      type="button"
                      className="tree-more-btn"
                      onClick={e => {
                        e.stopPropagation();
                        handleLoadMore(item.parentNode);
                      }}
                      tabIndex={0}
                      disabled={item.loading}
                      aria-label={`Show more in ${item.parentNode.name}`}
                    >
                      {item.loading ? (
                        <Loader2 size={13} className="spin" />
                      ) : (
                        <ChevronDown size={13} />
                      )}
                      Show more in {item.parentNode.name}
                    </button>
                    <span className="tree-toolbar-badge">
                      {item.loadedCount.toLocaleString()} of {item.totalCount.toLocaleString()}
                    </span>
                  </div>
                );
              }

              if (item.type === 'loading') {
                return (
                  <div
                    key={`loading-${item.parentNode.id}`}
                    role="none"
                    className="tree-status-row"
                    style={{ paddingLeft: 16 + item.depth * 18 }}
                  >
                    <span className="tree-loading-inline">
                      <Loader2 size={13} className="spin" />
                      Loading {item.parentNode.name}…
                    </span>
                  </div>
                );
              }

              if (item.type === 'empty') {
                return (
                  <div
                    key={`empty-${item.parentNode.id}`}
                    role="none"
                    className="tree-status-row"
                    style={{ paddingLeft: 16 + item.depth * 18 }}
                  >
                    <span className="tree-empty-text">Empty folder</span>
                  </div>
                );
              }

              if (item.type === 'error') {
                return (
                  <div
                    key={`error-${item.parentNode.id}`}
                    role="none"
                    className="tree-status-row"
                    style={{ paddingLeft: 16 + item.depth * 18 }}
                  >
                    <TriangleAlert size={14} style={{ color: '#d6a26d' }} />
                    <span style={{ color: '#d39578' }}>{item.error}</span>
                    <button
                      type="button"
                      className="tree-retry-btn"
                      onClick={e => {
                        e.stopPropagation();
                        handleRetry(item.parentNode.id);
                      }}
                      tabIndex={0}
                      aria-label={`Retry loading ${item.parentNode.name}`}
                    >
                      <RotateCw size={11} />
                      Retry
                    </button>
                  </div>
                );
              }

              return null;
            })}
        </div>
      </div>
    </div>
  );
}
