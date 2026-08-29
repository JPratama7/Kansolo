import { For, Show, createEffect, createSignal } from 'solid-js';
import { Portal } from 'solid-js/web';
import { Dialog } from '@ark-ui/solid/dialog';
import type { SourceInstance } from '../types.ts';
import { deleteAllSourceCards, listSources } from '../db.ts';
import { reload } from './Board.tsx';
import { toaster } from './ui/toaster.ts';

interface ClearSourceModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export default function ClearSourceModal(props: ClearSourceModalProps) {
  const [sources, setSources] = createSignal<SourceInstance[]>([]);
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);

  // (Re)load sources each time the dialog opens.
  createEffect(() => {
    if (!props.open) return;
    void (async () => {
      try {
        setSources(await listSources());
      } catch (e) {
        // Action-result error → toast (decision 8).
        toaster.error({
          title: 'Could not load sources',
          description: e instanceof Error ? e.message : String(e),
        });
      }
    })();
  });

  const selected = () => sources().find((s) => s.id === selectedId()) ?? null;

  async function handleConfirm() {
    const src = selected();
    if (!src) {
      // Form validation → inline `<p role="alert">` (decision 8).
      setError('Pick a source to clear.');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await deleteAllSourceCards(src.sourceType);
      await reload();
      props.onOpenChange(false);
      toaster.success({ title: 'Source cleared', description: src.label });
    } catch (e) {
      // Action-result error → toast (decision 8).
      toaster.error({
        title: 'Clear failed',
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog.Root
      open={props.open}
      lazyMount
      unmountOnExit
      closeOnEscape
      closeOnInteractOutside
      aria-label="Clear source cards"
      onOpenChange={(e) => props.onOpenChange(e.open)}
    >
      <Portal>
        <Dialog.Backdrop class="fixed inset-0 z-50 bg-black/50" />
        <Dialog.Positioner class="fixed inset-0 z-50 flex items-start justify-center pt-16 px-4">
          <Dialog.Content
            class="w-full max-w-md max-h-[85vh] overflow-y-auto board-scroll bg-surface rounded-[var(--radius-card)] border border-border-subtle shadow-2xl"
            aria-label="Clear source cards"
          >
            <div class="sticky top-0 flex items-center justify-between px-4 py-3 bg-surface border-b border-border-subtle">
              <h2 class="text-base font-bold text-ink">Clear source cards</h2>
              <button
                type="button"
                class="text-xl text-ink-secondary hover:text-ink leading-none px-1"
                aria-label="Close"
                onClick={() => props.onOpenChange(false)}
              >
                ×
              </button>
            </div>

            <div class="p-4 flex flex-col gap-3">
              <p class="text-xs text-ink-secondary">
                Pick a source to delete every card it sourced plus its sync snapshots. Local cards stay.
              </p>

              <Show when={sources().length === 0}>
                <p class="text-sm text-ink-secondary">No sources configured.</p>
              </Show>

              <ul class="flex flex-col gap-1">
                <For each={sources()}>
                  {(src) => (
                    <li>
                      <label
                        class="flex items-center gap-2 text-sm text-ink rounded px-2 py-1.5 border cursor-pointer"
                        classList={{
                          'border-accent bg-accent/10': selectedId() === src.id,
                          'border-border-subtle': selectedId() !== src.id,
                        }}
                      >
                        <input
                          type="radio"
                          name="clear-source"
                          value={src.id}
                          checked={selectedId() === src.id}
                          onChange={() => setSelectedId(src.id)}
                        />
                        <span class="min-w-0 truncate font-semibold">{src.label}</span>
                        <span class="text-[10px] font-mono text-ink-secondary bg-base/60 rounded px-1 py-0.5">
                          {src.sourceType}
                        </span>
                      </label>
                    </li>
                  )}
                </For>
              </ul>

              {error() && <p class="text-sm text-p-urgent" role="alert">{error()}</p>}

              <div class="flex gap-2 justify-end">
                <button
                  type="button"
                  class="px-3 py-1.5 text-sm font-medium rounded text-ink-secondary hover:bg-elevated transition-colors"
                  onClick={() => props.onOpenChange(false)}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  class="px-3 py-1.5 text-sm font-medium rounded bg-accent hover:bg-accent-hover text-base transition-colors disabled:opacity-50"
                  disabled={!selected() || busy()}
                  onClick={() => void handleConfirm()}
                >
                  {busy() ? 'Clearing…' : 'Clear'}
                </button>
              </div>
            </div>
          </Dialog.Content>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
  );
}
