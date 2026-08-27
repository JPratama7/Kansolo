import { test, vi } from 'vitest';
import { Show, createSignal } from 'solid-js';
import { fireEvent, render, waitFor, cleanup } from '@solidjs/testing-library';
import { Toaster, Toast } from '@ark-ui/solid/toast';
import EditModal from '../../EditModal.tsx';
import { toaster } from '../toaster.ts';
import { installDom, resetDom, teardownDom } from './setup.ts';
import type { KanbanCard, TreeSource } from '../../../types.ts';

installDom();

const card: KanbanCard = {
  id: 'c1', title: 'Original', description: '', priority: 'medium',
  column: 'backlog', source: 'local', position: 1,
  createdAt: '2024-01-01T00:00:00Z', updatedAt: '2024-01-01T00:00:00Z',
};
const treeSources: TreeSource[] = [];

function ToasterMount() {
  return (
    <Toaster toaster={toaster}>
      {(t) => (
        <Toast.Root data-type={t().type}>
          <Toast.Title>{t().title}</Toast.Title>
          <Toast.Description>{t().description}</Toast.Description>
          <Show when={t().action}>
            <Toast.ActionTrigger>{t().action?.label}</Toast.ActionTrigger>
          </Show>
        </Toast.Root>
      )}
    </Toaster>
  );
}

test('EditModal: dirty close shows confirmation toast, Discard closes modal', async () => {
  resetDom();
  toaster.dismiss();
  const internals: { invoke: (...a: unknown[]) => Promise<unknown>; convertFileSrc: (p: string) => string } = {
    invoke: () => Promise.resolve(),
    convertFileSrc: (p) => `asset://localhost/${p}`,
  };
  (globalThis.window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = internals;
  const invokeStub = vi.spyOn(internals, 'invoke').mockImplementation(() => Promise.resolve());

  const [open, setOpen] = createSignal(true);
  const { getByText, getByLabelText, queryByRole, baseElement } = render(() => (
    <>
      <EditModal
        card={card}
        treeSources={() => treeSources}
        open={open()}
        onOpenChange={setOpen}
        onSave={() => {}}
      />
      <ToasterMount />
    </>
  ));

  // Modal is rendered with open=true, but Ark UI uses lazyMount so the
  // portaled content appears after an effect flush. The form defaults to
  // preview mode, so we must switch to the Edit tab to reveal the Title
  // input before typing.
  await waitFor(() => {
    if (!baseElement.querySelector('[data-value="edit"]')) {
      throw new Error('EditModal form not mounted yet');
    }
  });
  // Click the Edit tab trigger directly (getByText is unreliable here because
  // Ark UI wraps the label in extra spans).
  fireEvent.click(baseElement.querySelector('[data-value="edit"]') as HTMLElement);
  await waitFor(() => {
    if (!baseElement.querySelector('#edit-title')) {
      throw new Error('Title input not visible after Edit tab');
    }
  });
  fireEvent.input(baseElement.querySelector('#edit-title') as HTMLInputElement, {
    target: { value: 'Changed title' },
  });

  // Trigger the guarded close via Escape. Ark UI's dismissable layer
  // attaches a capture-phase keydown listener on `document` (not the
  // content element), so we dispatch there. The EditModal's
  // onEscapeKeyDown calls preventDefault + requestClose(), which shows
  // the confirmation toast when dirty without closing the modal.
  fireEvent.keyDown(document, { key: 'Escape' });
  await waitFor(() => {
    if (!baseElement.textContent?.includes('Discard unsaved changes?')) {
      throw new Error('confirmation toast not rendered');
    }
  });
  // The dirty guard (requestClose) shows the toast and does NOT call
  // onOpenChange(false), so the controlled open signal stays true. Ark
  // UI's internal state may briefly flicker in happy-dom, but the
  // controlled prop is the source of truth.
  if (!open()) throw new Error('modal should stay open while dirty (open signal)');

  fireEvent.click(getByText('Discard', { selector: 'button' }));
  await waitFor(() => {
    if (open()) throw new Error('modal should close after discard (open signal)');
  });

  invokeStub.mockRestore();
  cleanup();
  teardownDom();
});
