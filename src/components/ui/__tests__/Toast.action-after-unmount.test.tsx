import { test, vi, expect } from 'vitest';
import { waitFor } from '@solidjs/testing-library';
import { installDom, resetDom, teardownDom } from './setup.ts';
import { render, cleanup } from '@solidjs/testing-library';
import { createSignal, Show } from 'solid-js';
import { Toaster, Toast } from '@ark-ui/solid/toast';
import { toaster } from '../toaster.ts';

installDom();

test('Toast action after originating component unmount: no crash, dismisses cleanly', async () => {
  const errorSpy = vi.spyOn(console, 'error');
  // Simulate the EditModal guard: card may be deleted while toast is shown.
  const [card, setCard] = createSignal<{ id: string } | null>({ id: 'c1' });

  // The Toaster must stay mounted — it renders the toast DOM. The
  // "originating component" (EditModal) is simulated by the `card` signal;
  // setting it to null mimics the card being deleted/unmounted while the
  // toast is still visible.
  render(() => (
    <Toaster toaster={toaster}>
      {(toast) => (
        <Toast.Root data-type={toast().type}>
          <Toast.Title>{toast().title}</Toast.Title>
          <Toast.Description>{toast().description}</Toast.Description>
          <Show when={toast().action}>
            <Toast.ActionTrigger>{toast().action?.label}</Toast.ActionTrigger>
          </Show>
          <Toast.CloseTrigger aria-label="Close" />
        </Toast.Root>
      )}
    </Toaster>
  ));

  const id = toaster.create({
    title: 'Discard?',
    type: 'warning',
    duration: Infinity,
    action: {
      label: 'Discard',
      onClick: () => {
        // Guard: originating card was deleted/unmounted while toast shown.
        if (card() === null) {
          toaster.dismiss(id);
          return;
        }
      },
    },
  });

  // Wait for the toast to render in the portal.
  await new Promise((r) => setTimeout(r, 10));

  // Simulate the originating component's data being gone (card deleted).
  setCard(null);

  // The action button renders as a <button> via Toast.ActionTrigger.
  // Query the portal (document.body) for the action-trigger button.
  const btn = document.body.querySelector('[data-part="action-trigger"]') as HTMLButtonElement | null;
  if (!btn) throw new Error('Discard action button not found in portal');
  btn.click();

  // Wait for the toast to be dismissed from the DOM (the dismiss is
  // async — the toast machine processes the DISMISS event and the
  // Toaster re-renders without the toast).
  await waitFor(() => {
    const toastStillPresent = document.body.textContent?.includes('Discard?') ?? false;
    if (toastStillPresent) throw new Error('toast should be dismissed after action click');
  });

  expect(errorSpy).not.toHaveBeenCalled();

  errorSpy.mockRestore();
  toaster.dismiss();
  resetDom();
  cleanup();
  teardownDom();
});
