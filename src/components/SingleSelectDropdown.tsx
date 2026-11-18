import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useEscCloseTopmost } from "../hooks/useEscClose";
import { Check, ChevronDown } from "lucide-react";
import "./SingleSelectDropdown.css";
import { dropdownLayer, dropdownPlacement, type DropdownPlacement } from "../utils/dropdownPlacement";

const ENABLED_OPTION_SELECTOR = '[role="option"]:not(:disabled)';

export interface SingleSelectOption {
  value: string;
  label: string;
  badge?: ReactNode;
  disabled?: boolean;
}

interface SingleSelectDropdownProps {
  value: string;
  options: SingleSelectOption[];
  onChange: (value: string) => void;
  className?: string;
  menuClassName?: string;
  disabled?: boolean;
  ariaLabel?: string;
  placeholder?: string;
  search?: { label: string; emptyLabel: string };
  menuPlacement?: "down" | "up";
  menuWidth?: number;
  menuMaxHeight?: number;
}

export function SingleSelectDropdown({
  value,
  options,
  onChange,
  className,
  menuClassName,
  disabled = false,
  ariaLabel,
  placeholder,
  search,
  menuPlacement = "down",
  menuWidth,
  menuMaxHeight = 280,
}: SingleSelectDropdownProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [menuStyle, setMenuStyle] = useState<DropdownPlacement | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const menuId = useId();
  const entryFocus = useRef<"selected" | "last">("selected");
  const searchRef = useRef<HTMLInputElement | null>(null);

  const closeMenu = () => { setOpen(false); triggerRef.current?.focus(); };
  useEscCloseTopmost(open, closeMenu, menuRef);
  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);

  const selectedOption = useMemo(
    () => options.find((option) => option.value === value) ?? null,
    [options, value],
  );

  useEffect(() => {
    if (!open) return;

    const updateMenuPosition = (event?: Event) => {
      const eventTarget = event?.target;
      if (
        event?.type === "scroll" &&
        eventTarget instanceof Node &&
        menuRef.current?.contains(eventTarget)
      ) {
        return;
      }
      const rect = triggerRef.current?.getBoundingClientRect();
      if (!rect) return;
      const layers: string[] = [];
      for (let ancestor: Element | null = triggerRef.current; ancestor; ancestor = ancestor.parentElement) {
        layers.push(window.getComputedStyle(ancestor).zIndex);
      }
      const nextStyle = dropdownPlacement(rect, { width: window.innerWidth, height: window.innerHeight },
        menuPlacement, menuWidth, menuMaxHeight, dropdownLayer(layers));
      setMenuStyle((prev) =>
        prev &&
        prev.top === nextStyle.top &&
        prev.bottom === nextStyle.bottom &&
        prev.left === nextStyle.left &&
        prev.width === nextStyle.width &&
        prev.maxHeight === nextStyle.maxHeight &&
        prev.zIndex === nextStyle.zIndex
          ? prev
          : nextStyle,
      );
    };

    const handlePointerDown = (event: MouseEvent) => {
      const target = event.target as Node | null;
      if (!target) return;
      if (rootRef.current?.contains(target)) return;
      if (menuRef.current?.contains(target)) return;
      setOpen(false);
    };

    updateMenuPosition();
    // 延迟绑定，避免与打开触发器同一次 pointer/click 立刻关掉菜单。
    const timer = window.setTimeout(() => {
      document.addEventListener("mousedown", handlePointerDown);
    }, 0);
    window.addEventListener("resize", updateMenuPosition);
    window.addEventListener("scroll", updateMenuPosition, true);

    return () => {
      window.clearTimeout(timer);
      document.removeEventListener("mousedown", handlePointerDown);
      window.removeEventListener("resize", updateMenuPosition);
      window.removeEventListener("scroll", updateMenuPosition, true);
    };
  }, [menuMaxHeight, menuPlacement, menuWidth, open]);

  useEffect(() => {
    if (!disabled) return;
    setOpen(false);
  }, [disabled]);

  const menuReady = menuStyle !== null;
  useEffect(() => {
    if (!open || !menuReady) return;
    if (searchRef.current) {
      searchRef.current.focus();
      return;
    }
    const items = menuRef.current?.querySelectorAll<HTMLButtonElement>(ENABLED_OPTION_SELECTOR);
    if (!items?.length) return;
    const selected = menuRef.current?.querySelector<HTMLButtonElement>(
      `${ENABLED_OPTION_SELECTOR}[aria-selected="true"]`,
    );
    (selected ?? items[entryFocus.current === "last" ? items.length - 1 : 0])?.focus();
  }, [open, menuReady]);

  function handleMenuKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape" || event.key === "Tab") {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
      }
      setOpen(false);
      // Tab continues from the trigger instead of jumping to the portal's position.
      triggerRef.current?.focus();
      return;
    }
    const items = Array.from(
      menuRef.current?.querySelectorAll<HTMLButtonElement>(ENABLED_OPTION_SELECTOR) ?? [],
    );
    // Typing in the search box stays untouched; only arrows leave it for the list.
    const inSearch = Boolean(searchRef.current) && document.activeElement === searchRef.current;
    if (inSearch) {
      if (event.nativeEvent.isComposing) return;
      if (event.key === "Enter") event.preventDefault();
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    }
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    if (!items.length) return;
    const index = items.findIndex((item) => item === document.activeElement);
    if (inSearch && event.key === "ArrowUp") {
      items[items.length - 1].focus();
      return;
    }
    if (!inSearch && searchRef.current && index === 0 && event.key === "ArrowUp") {
      searchRef.current.focus();
      return;
    }
    const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1
      : (index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
    items[next]?.focus();
  }

  const currentLabel = selectedOption?.label ?? placeholder ?? "";
  const normalizedQuery = search ? query.trim().toLowerCase() : "";
  const visibleOptions = normalizedQuery
    ? options.filter((option) => option.label.toLowerCase().includes(normalizedQuery))
    : options;

  return (
    <div
      ref={rootRef}
      className={[
        "single-select-dropdown",
        disabled ? "disabled" : "",
        className ?? "",
      ]
        .filter(Boolean)
        .join(" ")}
    >
      <button
        ref={triggerRef}
        type="button"
        className={`single-select-dropdown-trigger${open ? " open" : ""}`}
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open && menuStyle ? menuId : undefined}
        onKeyDown={(event) => {
          if (disabled || !["ArrowDown", "ArrowUp"].includes(event.key)) return;
          event.preventDefault();
          entryFocus.current = event.key === "ArrowUp" ? "last" : "selected";
          setOpen(true);
        }}
        onClick={() => {
          if (disabled) return;
          entryFocus.current = "selected";
          setOpen((prev) => !prev);
        }}
        disabled={disabled}
      >
        <span className="single-select-dropdown-value" title={currentLabel}>
          {currentLabel}
        </span>
        {selectedOption?.badge}
        <span className="single-select-dropdown-arrow">
          <ChevronDown size={16} />
        </span>
      </button>

      {open && menuStyle
        ? createPortal(
            <div
              ref={menuRef}
              className={[
                "single-select-dropdown-menu",
                search ? "searchable" : "",
                menuClassName ?? "",
              ]
                .filter(Boolean)
                .join(" ")}
              style={{
                position: "fixed",
                top: menuStyle.top !== undefined ? `${menuStyle.top}px` : "auto",
                bottom: menuStyle.bottom !== undefined ? `${menuStyle.bottom}px` : "auto",
                left: `${menuStyle.left}px`,
                width: `${menuStyle.width}px`,
                maxHeight: `${menuStyle.maxHeight}px`,
                zIndex: menuStyle.zIndex,
              }}
              onKeyDown={handleMenuKeyDown}
            >
              {search && (
                <div className="single-select-dropdown-search">
                  <input
                    ref={searchRef}
                    type="search"
                    value={query}
                    onChange={(event) => setQuery(event.target.value)}
                    placeholder={search.label}
                    aria-label={search.label}
                    aria-controls={menuId}
                  />
                </div>
              )}
              <div id={menuId} role="listbox" aria-label={ariaLabel}>
                {visibleOptions.map((option) => {
                  const active = option.value === value;
                  return (
                    <button
                      key={option.value}
                      type="button"
                      className={`single-select-dropdown-item${active ? " active" : ""}`}
                      disabled={option.disabled}
                      onClick={() => {
                        if (option.disabled) return;
                        onChange(option.value);
                        closeMenu();
                      }}
                      role="option"
                      tabIndex={-1}
                      aria-selected={active}
                      aria-disabled={option.disabled || undefined}
                    >
                      <span
                        className="single-select-dropdown-item-label"
                        title={option.label}
                      >
                        {option.label}
                      </span>
                      {option.badge}
                      <span className="single-select-dropdown-item-check">
                        {active ? <Check size={15} /> : null}
                      </span>
                    </button>
                  );
                })}
              </div>
              {search && visibleOptions.length === 0 && (
                <div className="single-select-dropdown-empty" role="status">{search.emptyLabel}</div>
              )}
            </div>,
            document.body,
          )
        : null}
    </div>
  );
}
