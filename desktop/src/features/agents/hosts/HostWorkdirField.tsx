import type * as React from "react";

import { Input } from "@/shared/ui/input";

/** Longest folder path the desktop accepts (mirrors the Rust check). */
export const HOST_WORKDIR_MAX_CHARS = 300;

export const HOST_WORKDIR_PLACEHOLDER = "~/buzz-agents/<agent> (default)";

/**
 * "Folder on <machine>": where an agent runs on its machine. Blank means the
 * machine's default folder. The path belongs to that machine, so nothing
 * here checks it against this computer.
 */
export function HostWorkdirField({
  id,
  testId,
  machineName,
  value,
  onChange,
  onKeyDown,
  disabled,
  extraHint,
  children,
}: {
  id: string;
  testId: string;
  machineName: string;
  value: string;
  onChange: (value: string) => void;
  onKeyDown?: React.KeyboardEventHandler<HTMLInputElement>;
  disabled?: boolean;
  /** An extra sentence shown under the standard helper. */
  extraHint?: string | null;
  /** Controls rendered beside the input (e.g. a Save button). */
  children?: React.ReactNode;
}) {
  const hintId = `${id}-hint`;
  return (
    <div className="space-y-1.5">
      <label className="text-sm font-medium" htmlFor={id}>
        Folder on {machineName}
      </label>
      <div className="flex gap-2">
        <Input
          aria-describedby={hintId}
          autoCapitalize="off"
          autoComplete="off"
          autoCorrect="off"
          className="min-w-0 flex-1 font-mono"
          data-testid={testId}
          disabled={disabled}
          id={id}
          maxLength={HOST_WORKDIR_MAX_CHARS}
          onChange={(event) => onChange(event.target.value)}
          onKeyDown={onKeyDown}
          placeholder={HOST_WORKDIR_PLACEHOLDER}
          spellCheck={false}
          value={value}
        />
        {children}
      </div>
      <p className="text-xs text-muted-foreground" id={hintId}>
        Path on that machine. ~ means its home folder. Created if missing.
        {extraHint ? ` ${extraHint}` : null}
      </p>
    </div>
  );
}
