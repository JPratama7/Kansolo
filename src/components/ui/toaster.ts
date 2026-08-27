import { createToaster } from '@ark-ui/solid/toast';

/**
 * Single toaster instance shared across the app. Mounted once at the App
 * root via `<Toaster toaster={toaster}>`. Confirmation toasts use
 * `duration: Infinity` with an action button; success/error toasts use
 * the default duration.
 */
export const toaster = createToaster({
  placement: 'bottom-end',
  max: 5,
  gap: 12,
});
