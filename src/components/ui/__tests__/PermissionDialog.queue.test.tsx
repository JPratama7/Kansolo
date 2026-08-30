import { test, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@solidjs/testing-library';
import PermissionDialog, {
  enqueuePermission,
  dequeuePermission,
  permissionHeadSignal,
  clearPermissionQueue,
} from '../../PermissionDialog.tsx';
import { installDom, resetDom, teardownDom } from './setup.ts';

installDom();

/** Mock Tauri invoke: only acp_respond_permission is used by the dialog. */
function mockInvoke() {
  const calls: { runId: string; requestId: string; approved: boolean }[] = [];
  // @ts-expect-error happy-dom window lacks Tauri internals typings
  globalThis.window.__TAURI_INTERNALS__ = {
    invoke: async (cmd: string, args: Record<string, unknown>) => {
      if (cmd === 'acp_respond_permission') {
        calls.push({
          runId: args.runId as string,
          requestId: args.requestId as string,
          approved: args.approved as boolean,
        });
        return null;
      }
      return null;
    },
    transformCallback: () => 0,
  };
  return calls;
}

const itemA = { runId: 'r1', requestId: 'r1:t1', description: 'Read: foo.txt', timeoutMs: 60_000 };
const itemB = { runId: 'r2', requestId: 'r2:t2', description: 'Write: bar.txt', timeoutMs: 60_000 };

beforeEach(() => {
  resetDom();
  clearPermissionQueue();
});

afterEach(() => {
  cleanup();
  clearPermissionQueue();
});

/** Two concurrent requests: only one dialog (the head) is rendered. */
test('PermissionDialog: two concurrent requests render one dialog', async () => {
  mockInvoke();
  render(() => <PermissionDialog />);

  enqueuePermission(itemA);
  enqueuePermission(itemB);

  // Head is A; its description is shown, B's is not. The dialog mounts via
  // Portal, so wait for the DOM rather than reading immediately.
  await waitFor(() => {
    const body = document.body.textContent ?? '';
    if (!body.includes('Read: foo.txt')) throw new Error('A description missing');
  });
  const body = document.body.textContent ?? '';
  if (body.includes('Write: bar.txt')) throw new Error('B leaked into dialog');

  // Exactly one dialog is rendered: only the head is mediated, so there
  // is exactly one Approve button in the DOM.
  const approveBtns = [...document.querySelectorAll('button')].filter((b) =>
    b.textContent?.trim() === 'Approve',
  );
  if (approveBtns.length !== 1) throw new Error(`expected 1 Approve btn, got ${approveBtns.length}`);
});

/** FIFO: enqueue A then B; head is A; dequeue A → head becomes B. */
test('PermissionDialog queue: FIFO order', () => {
  enqueuePermission(itemA);
  enqueuePermission(itemB);
  if (permissionHeadSignal()?.requestId !== 'r1:t1') throw new Error('head should be A');

  dequeuePermission('r1:t1');
  if (permissionHeadSignal()?.requestId !== 'r2:t2') throw new Error('head should advance to B');

  dequeuePermission('r2:t2');
  if (permissionHeadSignal() !== null) throw new Error('head should be null');
});

/** Dequeue on respond: clicking Approve dequeues + invokes the Rust command. */
test('PermissionDialog: dequeue on respond', async () => {
  const calls = mockInvoke();
  render(() => <PermissionDialog />);
  enqueuePermission(itemA);

  await waitFor(() => {
    if (permissionHeadSignal()?.requestId !== 'r1:t1') throw new Error('head not set');
  });

  const approveBtn = await waitFor(() => {
    const btn = [...document.querySelectorAll('button')].find((b) =>
      b.textContent?.trim() === 'Approve',
    );
    if (!btn) throw new Error('Approve button not found');
    return btn;
  });
  fireEvent.click(approveBtn);

  await waitFor(() => {
    if (permissionHeadSignal() !== null) throw new Error('head not cleared after respond');
  });
  if (calls.length !== 1 || calls[0].approved !== true) {
    throw new Error(`respond not forwarded, calls=${JSON.stringify(calls)}`);
  }
});

/** Dequeue on timeout: countdown reaching 0 dequeues the head. */
test('PermissionDialog: dequeue on timeout', async () => {
  mockInvoke();
  vi.useFakeTimers();
  try {
    render(() => <PermissionDialog />);
    enqueuePermission({ ...itemA, timeoutMs: 1000 });

    // Flush the reactive effect that starts the countdown interval.
    await vi.advanceTimersByTimeAsync(0);
    if (permissionHeadSignal()?.requestId !== 'r1:t1') throw new Error('head not set');

    // Advance past the 1s countdown; the interval tick computes left=0 and
    // dequeues the head.
    await vi.advanceTimersByTimeAsync(1100);
    if (permissionHeadSignal() !== null) throw new Error('head not cleared on timeout');
  } finally {
    vi.useRealTimers();
  }
});
