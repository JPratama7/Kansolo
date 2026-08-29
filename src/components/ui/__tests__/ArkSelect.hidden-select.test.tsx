import { test } from 'vitest';
import { render, cleanup } from '@solidjs/testing-library';
import { installDom, resetDom, teardownDom } from './setup.ts';
import { ArkSelect } from '../ArkSelect.tsx';

installDom();

function assertTrue(cond: unknown, msg: string): void {
  if (!cond) throw new Error(`assert failed: ${msg}`);
}

test('ArkSelect: HiddenSelect carries name + value for form submission', () => {
  resetDom();
  const onValueChange = () => {};

  const { container } = render(() => (
    <ArkSelect
      name="priority"
      value="high"
      items={[
        { label: 'High', value: 'high' },
        { label: 'Low', value: 'low' },
      ]}
      onValueChange={onValueChange}
    />
  ));

  const select = container.querySelector('select[name="priority"]') as HTMLSelectElement | null;
  assertTrue(select, 'hidden <select name="priority"> exists');
  assertTrue(select?.value === 'high', `select value is "high" (got "${select?.value}")`);

  const form = document.createElement('form');
  form.appendChild(select!);
  document.body.appendChild(form);

  const fd = new FormData(form);
  assertTrue(fd.get('priority') === 'high', `FormData priority === "high" (got "${fd.get('priority')}")`);

  form.remove();
  cleanup();
  teardownDom();
});
