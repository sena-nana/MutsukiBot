/**
 * Lilia-style searchable context menu host (vanilla JS mirror).
 *
 * Mirrors sena-nana/LiliaUI `ContextMenuHost`:
 *  - fixed-position menu anchored at (clientX, clientY), viewport-clamped;
 *  - searchable mode: top input, autofocus, flatten leaves on non-empty query;
 *  - empty query: top-level items, items with `children` open a right flyout submenu;
 *  - keyboard: ArrowUp/Down moves highlight (search mode), Enter selects, Escape closes;
 *  - closes on outside pointerdown, scroll, resize, blur, Escape.
 *
 * Item shape:
 *   { id?, label, disabled?, danger?, keywords?, children?, onSelect?, header?, meta? }
 * `header: true` items render as non-interactive section dividers and are skipped
 * during leaf flattening.
 */

const Z_INDEX = 2000;
const DEFAULT_SEARCH_PLACEHOLDER = "搜索";
const DEFAULT_EMPTY_TEXT = "没有匹配项";

let openState = null;

function esc(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function clamp(value, min, max) {
  return Math.min(Math.max(value, min), max);
}

function collectLeaves(items, groupLabel) {
  const leaves = [];
  for (const item of items || []) {
    if (item.header) continue;
    if (item.children?.length) {
      leaves.push(...collectLeaves(item.children, item.label));
      continue;
    }
    leaves.push({ item, groupLabel });
  }
  return leaves;
}

function matchLeaf(leaf, query) {
  const tokens = [leaf.item.label, leaf.item.id, leaf.groupLabel, ...(leaf.item.keywords || [])]
    .filter(Boolean)
    .join(" ")
    .toLocaleLowerCase();
  return tokens.includes(query);
}

function itemClass(item, active) {
  return [
    "ctx-menu__item",
    active && "ctx-menu__item--active",
    item.danger && "ctx-menu__item--danger",
    item.disabled && "ctx-menu__item--disabled",
  ].filter(Boolean).join(" ");
}

function renderItem(item, { active = false, arrow = false, meta = "" } = {}) {
  const arrowMarkup = arrow ? `<span class="ctx-menu__arrow" aria-hidden="true">›</span>` : "";
  return `<button type="button" class="${itemClass(item, active)}" ${item.disabled ? "disabled" : ""}>
    <span class="ctx-menu__label">${esc(item.label)}</span>${meta}${arrowMarkup}
  </button>`;
}

function renderHeader(item) {
  return `<div class="ctx-menu__header" aria-hidden="true">${esc(item.label)}</div>`;
}

function renderEntry(item, opts) {
  return item.header ? renderHeader(item) : renderItem(item, opts);
}

function place(el, x, y) {
  const { width, height } = el.getBoundingClientRect();
  const margin = 4;
  el.style.left = `${clamp(x, margin, Math.max(margin, window.innerWidth - width - margin))}px`;
  el.style.top = `${clamp(y, margin, Math.max(margin, window.innerHeight - height - margin))}px`;
}

function placeSubmenu(sub, parentRect) {
  const { width, height } = sub.getBoundingClientRect();
  const margin = 4;
  const placeLeft = window.innerWidth - parentRect.right < width + margin && parentRect.left > width + margin;
  const left = placeLeft ? parentRect.left - width + 1 : parentRect.right - 1;
  sub.style.left = `${left}px`;
  sub.style.top = `${clamp(parentRect.top - 4, margin, window.innerHeight - height - margin)}px`;
}

function closeContextMenu() {
  openState?.dispose();
  openState = null;
}

function openContextMenuAt(clientX, clientY, items, options = {}) {
  closeContextMenu();
  if (!items?.length) return;

  const searchable = Boolean(options.searchable);
  const root = document.createElement("div");
  root.className = `ctx-menu${searchable ? " ctx-menu--searchable" : ""}`;
  root.style.zIndex = String(Z_INDEX);
  const scroll = document.createElement("div");
  scroll.className = "ctx-menu__scroll";
  root.appendChild(scroll);
  let input = null;
  if (searchable) {
    const search = document.createElement("div");
    search.className = "ctx-menu__search";
    search.innerHTML = `<input type="search" placeholder="${esc(options.searchPlaceholder || DEFAULT_SEARCH_PLACEHOLDER)}" />`;
    root.insertBefore(search, scroll);
    input = search.querySelector("input");
  }
  document.body.appendChild(root);
  place(root, clientX, clientY);

  const state = {
    items, options, root, scroll, input,
    query: "", activeIndex: 0, matches: [],
    activeSubmenuIndex: null, submenuEl: null, disposed: false,
  };

  function dispose() {
    if (state.disposed) return;
    state.disposed = true;
    window.removeEventListener("pointerdown", onGlobalPointerDown, true);
    window.removeEventListener("keydown", onGlobalKeydown);
    window.removeEventListener("scroll", onGlobalScroll, true);
    window.removeEventListener("resize", closeContextMenu);
    window.removeEventListener("blur", closeContextMenu);
    root.remove();
  }

  function onGlobalPointerDown(event) {
    if (!root.contains(event.target)) closeContextMenu();
  }

  function onGlobalKeydown(event) {
    if (event.key === "Escape") {
      event.stopPropagation();
      closeContextMenu();
    }
  }

  function onGlobalScroll(event) {
    if (!(event.target instanceof Node && root.contains(event.target))) closeContextMenu();
  }

  function isSearching() {
    return searchable && state.query.trim().length > 0;
  }

  function clearSubmenu() {
    state.activeSubmenuIndex = null;
    state.submenuEl?.remove();
    state.submenuEl = null;
  }

  function selectLeaf(item) {
    if (!item || item.disabled || item.header || item.children?.length) return;
    closeContextMenu();
    item.onSelect?.();
  }

  function bindLeaves(container, children) {
    const buttons = container.querySelectorAll(".ctx-menu__item");
    let buttonIndex = 0;
    for (const item of children) {
      if (item.header) continue;
      const button = buttons[buttonIndex++];
      if (button && !item.disabled) {
        button.addEventListener("click", () => selectLeaf(item));
      }
    }
  }

  function openSubmenu(index, parentEntry) {
    clearSubmenu();
    const parent = state.items[index];
    if (!parent?.children?.length || parent.disabled) return;
    state.activeSubmenuIndex = index;
    const sub = document.createElement("div");
    sub.className = "ctx-menu__submenu";
    sub.style.zIndex = String(Z_INDEX + 1);
    sub.innerHTML = parent.children.map((child) => renderEntry(child)).join("");
    root.appendChild(sub);
    state.submenuEl = sub;
    placeSubmenu(sub, parentEntry.getBoundingClientRect());
    bindLeaves(sub, parent.children);
  }

  function renderTopLevel() {
    scroll.innerHTML = state.items
      .map((item, index) => renderEntry(item, {
        arrow: Boolean(item.children?.length),
        active: state.activeSubmenuIndex === index,
      }))
      .join("");
    scroll.querySelectorAll(".ctx-menu__item").forEach((button, index) => {
      const item = state.items[index];
      if (!item || item.disabled) return;
      button.addEventListener("pointerenter", () => {
        if (item.children?.length) openSubmenu(index, button);
        else clearSubmenu();
      });
      button.addEventListener("click", () => {
        if (!item.children?.length) selectLeaf(item);
      });
    });
  }

  function renderSearchResults() {
    const query = state.query.trim().toLocaleLowerCase();
    state.matches = isSearching() ? collectLeaves(state.items).filter((leaf) => matchLeaf(leaf, query)) : [];
    if (!state.matches.length) {
      scroll.innerHTML = `<p class="ctx-menu__empty">${esc(options.emptyText || DEFAULT_EMPTY_TEXT)}</p>`;
      return;
    }
    if (state.activeIndex >= state.matches.length) state.activeIndex = 0;
    scroll.innerHTML = state.matches
      .map((leaf, index) => {
        const meta = leaf.groupLabel ? `<span class="ctx-menu__meta">${esc(leaf.groupLabel)}</span>` : "";
        return renderItem(leaf.item, { active: index === state.activeIndex, meta });
      })
      .join("");
    scroll.querySelectorAll(".ctx-menu__item").forEach((button, index) => {
      const leaf = state.matches[index];
      if (leaf && !leaf.item.disabled) {
        button.addEventListener("pointerenter", () => {
          state.activeIndex = index;
          updateSearchHighlight();
        });
        button.addEventListener("click", () => selectLeaf(leaf.item));
      }
    });
  }

  function updateSearchHighlight() {
    scroll.querySelectorAll(".ctx-menu__item").forEach((button, index) => {
      button.classList.toggle("ctx-menu__item--active", index === state.activeIndex);
    });
  }

  function render() {
    clearSubmenu();
    if (isSearching()) renderSearchResults();
    else renderTopLevel();
  }

  function onSearchInput(event) {
    state.query = event.target.value;
    state.activeIndex = 0;
    render();
  }

  function onSearchKeydown(event) {
    if (!isSearching()) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") event.preventDefault();
      return;
    }
    const count = state.matches.length;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!count) return;
      state.activeIndex = (state.activeIndex + (event.key === "ArrowDown" ? 1 : -1) + count) % count;
      updateSearchHighlight();
    } else if (event.key === "Enter") {
      event.preventDefault();
      selectLeaf(state.matches[state.activeIndex]?.item);
    }
  }

  if (input) {
    input.addEventListener("input", onSearchInput);
    input.addEventListener("keydown", onSearchKeydown);
  }

  window.addEventListener("pointerdown", onGlobalPointerDown, true);
  window.addEventListener("keydown", onGlobalKeydown);
  window.addEventListener("scroll", onGlobalScroll, true);
  window.addEventListener("resize", closeContextMenu);
  window.addEventListener("blur", closeContextMenu);

  render();
  if (input) requestAnimationFrame(() => { input.focus(); input.select(); });

  state.dispose = dispose;
  openState = state;
  return state;
}

export { openContextMenuAt, closeContextMenu, collectLeaves, matchLeaf };
