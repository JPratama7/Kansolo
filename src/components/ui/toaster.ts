import { createToaster } from "@ark-ui/solid/toast";

/** App-wide toaster. Confirmation toasts use `duration: Infinity`. */
export const toaster = createToaster({
  placement: "bottom-end",
  max: 5,
  gap: 12,
});
