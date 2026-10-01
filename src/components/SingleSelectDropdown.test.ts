import assert from 'node:assert/strict';
import test from 'node:test';
import { loadHookModule } from '../../tests/helpers/reactHookHarness';

interface Element { type: unknown; props: Record<string, any> }
function nodes(tree: unknown): Element[] {
  if (Array.isArray(tree)) return tree.flatMap(nodes);
  if (!tree || typeof tree !== 'object' || !('props' in tree)) return [];
  const element = tree as Element;
  return [element, ...nodes(element.props.children)];
}

function harness(value = 'b') {
  const listeners = new Map<string, (event: any) => void>();
  const document = { activeElement: null as FakeNode | null, body: {},
    addEventListener: (name: string, fn: (event: any) => void) => listeners.set(name, fn),
    removeEventListener: (name: string) => listeners.delete(name),
  };
  class FakeNode {
    children: FakeNode[] = [];
    selected = false;
    focus() { document.activeElement = this; }
    contains(node: FakeNode) { return this === node || this.children.includes(node); }
    getBoundingClientRect() { return { top: 40, bottom: 76, left: 200, width: 232 }; }
    querySelectorAll() { return this.children; }
    querySelector() { return this.children.find((child) => child.selected) ?? null; }
  }
  const root = new FakeNode(), trigger = new FakeNode(), menu = new FakeNode();
  const items = [new FakeNode(), new FakeNode(), new FakeNode()];
  root.children = [trigger]; menu.children = items;
  const calls: string[] = [];
  const props = { value, options: [{ value: 'a', label: 'Alpha' }, { value: 'b', label: 'Beta' }, { value: 'c', label: 'Gamma' }],
    onChange(next: string) { props.value = next; calls.push(next); }, disabled: false, ariaLabel: 'Provider' };
  const h = loadHookModule(new URL('./SingleSelectDropdown.tsx', import.meta.url), {
    'react-dom': { createPortal: (element: unknown) => element },
  }, { document, Node: FakeNode, window: { innerWidth: 1200, innerHeight: 800,
    setTimeout(fn: () => void) { fn(); return 0; }, clearTimeout() {}, addEventListener() {}, removeEventListener() {},
  } });
  h.render(() => {
    const tree = h.exports.SingleSelectDropdown(props);
    const elements = nodes(tree);
    elements.find((n) => n.type === 'div' && n.props.ref)!.props.ref.current = root;
    elements.find((n) => n.type === 'button' && n.props['aria-haspopup'])!.props.ref.current = trigger;
    const listbox = elements.find((n) => n.props.role === 'listbox');
    if (listbox) listbox.props.ref.current = menu;
    elements.filter((n) => n.props.role === 'option').forEach((n, index) => { items[index].selected = n.props['aria-selected']; });
    return tree;
  });
  const button = () => nodes(h.flush()).find((n) => n.type === 'button' && n.props['aria-haspopup'])!;
  const listbox = () => nodes(h.flush()).find((n) => n.props.role === 'listbox');
  function key(element: Element, name: string) {
    const event = { key: name, prevented: false, stopped: false,
      preventDefault() { this.prevented = true; }, stopPropagation() { this.stopped = true; } };
    element.props.onKeyDown(event); h.flush(); return event;
  }
  return { h, props, button, listbox, key, items, calls, document, trigger, listeners,
    open() { button().props.onClick(); h.flush(); },
  };
}

test('select opens with the selected option focused and exposes a linked listbox', () => {
  const h = harness(); h.open();
  assert.equal(h.document.activeElement, h.items[1]);
  assert.equal(h.button().props['aria-controls'], h.listbox()!.props.id);
  assert.equal(h.button().props['aria-expanded'], true);
  h.h.unmount(); assert.equal(h.listeners.size, 0);
});

test('arrow keys, Home and End navigate without changing the selection', () => {
  const h = harness(); h.key(h.button(), 'ArrowDown');
  h.key(h.listbox()!, 'ArrowDown'); assert.equal(h.document.activeElement, h.items[2]);
  h.key(h.listbox()!, 'ArrowDown'); assert.equal(h.document.activeElement, h.items[0]);
  h.key(h.listbox()!, 'ArrowUp'); assert.equal(h.document.activeElement, h.items[2]);
  h.key(h.listbox()!, 'Home'); assert.equal(h.document.activeElement, h.items[0]);
  h.key(h.listbox()!, 'End'); assert.equal(h.document.activeElement, h.items[2]);
  assert.deepEqual(h.calls, []); h.h.unmount();
});

test('ArrowUp falls back to the last option when no value is selected', () => {
  const h = harness('missing'); h.key(h.button(), 'ArrowUp');
  assert.equal(h.document.activeElement, h.items[2]); h.h.unmount();
});

test('Escape dismisses without selecting and restores focus to the trigger', () => {
  const h = harness(); h.open(); const event = h.key(h.listbox()!, 'Escape');
  assert.equal(event.prevented, true); assert.equal(event.stopped, true);
  assert.equal(h.listbox(), undefined); assert.equal(h.document.activeElement, h.trigger);
  assert.deepEqual(h.calls, []); h.h.unmount();
});

test('Tab closes from the trigger without preventing normal keyboard traversal', () => {
  const h = harness(); h.open(); const event = h.key(h.listbox()!, 'Tab');
  assert.equal(event.prevented, false); assert.equal(h.listbox(), undefined);
  assert.equal(h.document.activeElement, h.trigger); h.h.unmount();
});

test('selection commits once, closes and restores focus; outside clicks only dismiss', () => {
  const h = harness(); h.open();
  nodes(h.h.flush()).filter((n) => n.props.role === 'option')[2].props.onClick();
  assert.equal(h.listbox(), undefined); assert.deepEqual(h.calls, ['c']);
  assert.equal(h.document.activeElement, h.trigger);
  h.open(); h.listeners.get('mousedown')!({ target: {} });
  assert.equal(h.listbox(), undefined); assert.deepEqual(h.calls, ['c']); h.h.unmount();
});

test('disabled controls do not open and disabling an open control dismisses it', () => {
  const h = harness(); h.props.disabled = true; h.open();
  assert.equal(h.listbox(), undefined);
  h.key(h.button(), 'ArrowDown'); assert.equal(h.listbox(), undefined);
  h.props.disabled = false; h.open(); assert.ok(h.listbox());
  h.props.disabled = true; assert.equal(h.listbox(), undefined); h.h.unmount();
});
