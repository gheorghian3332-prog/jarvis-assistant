import type { ActionInfo, AuditEntry, Risk } from "./api";
import {
  MAX_ALIASES,
  MAX_ROOTS,
  validAliasName,
  validAliasTarget,
  validDrivePath,
  type Validation,
} from "./settings";

/** CSS class that gives the risk its colour. */
export function riskClass(risk: Risk): string {
  return `risk risk-${risk}`;
}

/** Whether the action is on, given the stored list of switched-off actions. Forbidden is never on. */
export function isEnabled(action: Pick<ActionInfo, "name" | "risk">, disabled: readonly string[]): boolean {
  return action.risk !== "forbidden" && !disabled.includes(action.name);
}

/** The new list of switched-off actions after a toggle (sorted, without duplicates). */
export function toggleAction(disabled: readonly string[], name: string, enabled: boolean): string[] {
  const rest = disabled.filter((item) => item !== name);
  return (enabled ? rest : [...rest, name]).sort();
}

export function addRoot(roots: readonly string[], input: string): Validation<string[]> {
  const path = input.trim();
  if (!validDrivePath(path)) return { ok: false, error: "D:\\Folder" };
  if (roots.some((root) => root.toLowerCase() === path.toLowerCase())) return { ok: false, error: path };
  if (roots.length >= MAX_ROOTS) return { ok: false, error: `max ${MAX_ROOTS}` };
  return { ok: true, value: [...roots, path] };
}

export function removeRoot(roots: readonly string[], path: string): string[] {
  return roots.filter((root) => root !== path);
}

export function addAlias(
  aliases: Readonly<Record<string, string>>,
  nameInput: string,
  targetInput: string,
): Validation<Record<string, string>> {
  const name = nameInput.trim();
  const target = targetInput.trim();
  if (!validAliasName(name)) return { ok: false, error: "name" };
  if (!validAliasTarget(target)) return { ok: false, error: "target" };
  if (!(name in aliases) && Object.keys(aliases).length >= MAX_ALIASES) {
    return { ok: false, error: `max ${MAX_ALIASES}` };
  }
  return { ok: true, value: { ...aliases, [name]: target } };
}

export function removeAlias(aliases: Readonly<Record<string, string>>, name: string): Record<string, string> {
  return Object.fromEntries(Object.entries(aliases).filter(([key]) => key !== name));
}

/** The newest `limit` audit entries (the backend already returns them newest first). */
export function recentActions(entries: readonly AuditEntry[], limit = 20): AuditEntry[] {
  return entries.slice(0, limit);
}
