import { createListCollection, Select } from "@ark-ui/solid/select";
import { createMemo, For, type JSX } from "solid-js";

export interface ArkSelectItem {
  label: string;
  value: string;
}

interface ArkSelectProps {
  items: ArkSelectItem[];
  value: string;
  onValueChange: (value: string) => void;
  placeholder?: string;
  name?: string;
  class?: string;
}

/** Reactive Ark UI Select wrapper; rebuilds its collection when `items` change. */
export function ArkSelect(props: ArkSelectProps): JSX.Element {
  const collection = createMemo(() =>
    createListCollection({ items: props.items })
  );
  return (
    <Select.Root
      collection={collection()}
      value={props.value ? [props.value] : []}
      onValueChange={(e) => props.onValueChange(e.value[0] ?? "")}
    >
      <Select.HiddenSelect name={props.name} />
      <Select.Trigger
        class={`flex items-center justify-between gap-2 ${props.class ?? ""}`}
      >
        <Select.ValueText placeholder={props.placeholder} />
        <Select.Indicator>
          <span class="i-carat-down" aria-hidden="true">▾</span>
        </Select.Indicator>
      </Select.Trigger>
      <Select.Positioner>
        <Select.Content class="bg-surface border border-border-subtle rounded-[var(--radius-card)] shadow-2xl py-1 max-h-60 overflow-y-auto z-50">
          <For each={collection().items}>
            {(item) => (
              <Select.Item
                item={item}
                class="w-full text-left text-sm text-ink px-3 py-1.5 hover:bg-elevated transition-colors cursor-pointer"
              >
                <Select.ItemText>{item.label}</Select.ItemText>
                <Select.ItemIndicator class="float-right">
                  ✓
                </Select.ItemIndicator>
              </Select.Item>
            )}
          </For>
        </Select.Content>
      </Select.Positioner>
    </Select.Root>
  );
}
