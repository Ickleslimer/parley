export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  options: {
    className?: string;
    id?: string;
    text?: string;
    attrs?: Record<string, string | number | boolean | undefined>;
    children?: Array<Node | string | null | undefined | false>;
  } = {},
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (options.className) {
    node.className = options.className;
  }
  if (options.id) {
    node.id = options.id;
  }
  if (options.text != null) {
    node.textContent = options.text;
  }
  if (options.attrs) {
    for (const [key, value] of Object.entries(options.attrs)) {
      if (value === undefined || value === false) {
        continue;
      }
      if (value === true) {
        node.setAttribute(key, "");
      } else {
        node.setAttribute(key, String(value));
      }
    }
  }
  if (options.children) {
    for (const child of options.children) {
      if (child === null || child === undefined || child === false) {
        continue;
      }
      node.append(typeof child === "string" ? document.createTextNode(child) : child);
    }
  }
  return node;
}

export function setText(node: Node, text: string): void {
  if (node.textContent !== text) {
    node.textContent = text;
  }
}

export function button(
  label: string,
  className: string,
  onClick: () => void,
  attrs: Record<string, string | number | boolean | undefined> = {},
): HTMLButtonElement {
  const node = el("button", {
    className,
    text: label,
    attrs: { type: "button", ...attrs },
  });
  node.addEventListener("click", onClick);
  return node;
}

export function labelledControl(
  labelText: string,
  control: HTMLElement,
  className = "field",
): HTMLLabelElement {
  const label = el("label", { className });
  label.append(el("span", { className: "field-label", text: labelText }), control);
  return label;
}
