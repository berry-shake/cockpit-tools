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
    '../hooks/useEscClose': { useEscCloseTopmost() {} },
  }, { document, Node: FakeNode, window: { innerWidth: 1200, innerHeight: 800,
    getComputedStyle: () => ({ zIndex: 'auto' }),
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

test('portaled dialog menus stay above their overlay and return focus after keyboard or selection', () => {
  const document: any = { body: {}, activeElement: null, addEventListener() {}, removeEventListener() {} };
  const chosen: string[] = [];
  let escape: (() => void) | undefined;
  let escapeRef: any;
  let triggerFocus = 0;
  const h = loadHookModule(new URL('./SingleSelectDropdown.tsx', import.meta.url), {
    'react-dom': { createPortal: (tree: unknown) => tree },
    '../hooks/useEscClose': { useEscCloseTopmost: (open: boolean, close: () => void, ref: any) => { escape = open ? close : undefined; escapeRef = ref; } },
  }, { document, Node: class {}, window: {
    innerWidth: 900, innerHeight: 600,
    getComputedStyle: (element: any) => ({ zIndex: element.layer ?? 'auto' }),
    setTimeout: () => 1, clearTimeout() {}, addEventListener() {}, removeEventListener() {},
  } });
  let tree = h.render(() => h.exports.SingleSelectDropdown({ value: '0', options: [{ value: '0', label: 'Client' }, { value: '1', label: 'Upstream' }], onChange: (value: string) => chosen.push(value) }));
  const trigger = tree.props.children[0];
  trigger.props.ref.current = {
    parentElement: { layer: '12040', parentElement: null },
    getBoundingClientRect: () => ({ left: 200, top: 510, bottom: 556, width: 300 }),
    focus: () => { triggerFocus++; },
  };
  trigger.props.onKeyDown({ key: 'ArrowDown', preventDefault() {} }); tree = h.flush();
  let menu = tree.props.children[1];
  assert.equal(escapeRef, menu.props.ref);
  assert.equal(menu.props.style.zIndex, 12041);
  assert.equal(menu.props.style.bottom, '100px');
  const items = [0, 1].map((id) => ({ focus: () => { document.activeElement = items[id]; } }));
  menu.props.ref.current = { querySelectorAll: () => items, querySelector: () => items[0] };
  document.activeElement = items[0];
  menu.props.onKeyDown({ key: 'ArrowDown', preventDefault() {} });
  assert.equal(document.activeElement, items[1]);
  menu.props.children[1].props.onClick(); tree = h.flush();
  assert.deepEqual(chosen, ['1']);
  assert.equal(tree.props.children[1], null);
  assert.equal(triggerFocus, 1);
  tree.props.children[0].props.onClick(); tree = h.flush();
  escape?.(); tree = h.flush();
  assert.equal(tree.props.children[1], null);
  assert.equal(triggerFocus, 2);
  h.unmount();
});
