export {};

declare module "solid-js" {
  namespace JSX {
    interface Directives {
      draggable: (el: HTMLElement, accessor: () => unknown) => void;
      droppable: (el: HTMLElement, accessor: () => unknown) => void;
    }
  }
}
