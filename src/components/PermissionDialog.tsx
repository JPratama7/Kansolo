import { Show, createEffect, createSignal, onCleanup } from 'solid-js';
import { Portal } from 'solid-js/web';
import { Dialog } from '@ark-ui/solid/dialog';

const PERMISSION_TIMEOUT_MS = 300_000; // 5 minutes (decision 29)

export interface PermissionDialogProps {
  open: boolean;
  /** Human-readable description of what the agent wants to do. */
  description: string;
  /** Called when user approves. */
  onApprove: () => void;
  /** Called when user denies. */
  onDeny: () => void;
}

/** Permission mediation UI. Shows when an agent sends a permission request.
 * Includes disclaimer: "Agent has full fs/network access inside its
 * worktree CWD" (decision 13). 5min timeout auto-deny (decision 29). */
export default function PermissionDialog(props: PermissionDialogProps) {
  const [remaining, setRemaining] = createSignal(PERMISSION_TIMEOUT_MS);

  let timer: ReturnType<typeof setInterval> | undefined;
  createEffect(() => {
    if (props.open) {
      setRemaining(PERMISSION_TIMEOUT_MS);
      const start = Date.now();
      timer = setInterval(() => {
        const elapsed = Date.now() - start;
        const left = Math.max(0, PERMISSION_TIMEOUT_MS - elapsed);
        setRemaining(left);
        if (left === 0) {
          props.onDeny();
        }
      }, 1000);
    } else {
      clearInterval(timer);
    }
  });

  onCleanup(() => clearInterval(timer));

  const secondsLeft = () => Math.ceil(remaining() / 1000);

  return (
    <Dialog.Root
      open={props.open}
      lazyMount
      unmountOnExit
      closeOnEscape
      closeOnInteractOutside={false}
      onOpenChange={(e) => { if (!e.open) props.onDeny(); }}
    >
      <Show when={props.open}>
        <Portal>
          <Dialog.Backdrop class="fixed inset-0 z-[60] bg-black/50" />
          <Dialog.Positioner class="fixed inset-0 z-[60] flex items-center justify-center px-4">
            <Dialog.Content class="relative w-full max-w-md bg-surface rounded-[var(--radius-card)] border border-border-subtle shadow-2xl p-5">
              <Dialog.Title class="text-base font-bold text-ink mb-2">
                Permission Request
              </Dialog.Title>
              <Dialog.Description class="text-sm text-ink-secondary mb-4">
                {props.description}
              </Dialog.Description>
              <div class="rounded border border-p-urgent/40 bg-p-urgent/10 p-3 mb-4">
                <p class="text-xs text-p-urgent font-medium">
                  Warning: The agent has full filesystem and network access
                  inside its worktree directory. Approve only if you trust
                  this action.
                </p>
              </div>
              <div class="flex items-center justify-between gap-3">
                <span class="text-xs text-ink-secondary">
                  Auto-deny in {secondsLeft()}s
                </span>
                <div class="flex gap-2">
                  <button
                    type="button"
                    class="px-3 py-1.5 text-sm font-medium rounded border border-border-subtle text-ink hover:bg-elevated transition-colors"
                    onClick={props.onDeny}
                  >
                    Deny
                  </button>
                  <button
                    type="button"
                    class="px-3 py-1.5 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors"
                    onClick={props.onApprove}
                  >
                    Approve
                  </button>
                </div>
              </div>
            </Dialog.Content>
          </Dialog.Positioner>
        </Portal>
      </Show>
    </Dialog.Root>
  );
}
