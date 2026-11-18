/** Turns a key press into a global shortcut string such as `F8` or `Ctrl+Alt+K`; `null` for a lone modifier. */
export function shortcutFromKeyEvent(event: {
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}): string | null {
  const code = event.code;
  if (/^(Control|Alt|Shift|Meta|OS)(Left|Right)$/.test(code) || code === "") return null;
  let key = code;
  const letter = /^Key([A-Z])$/.exec(code);
  const digit = /^Digit([0-9])$/.exec(code);
  if (letter?.[1]) key = letter[1];
  else if (digit?.[1]) key = digit[1];
  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push("Super");
  parts.push(key);
  return parts.join("+");
}
