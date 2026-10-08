import { PresenceDot } from "@/features/presence/ui/PresenceBadge";
import type { PersonaDropdownOption } from "./agentConfigOptions";

/**
 * Option label with the harness's optional description as a muted second line.
 *
 * `reservePresenceSlot`: when any option in the menu shows a presence dot,
 * every option (and the footer action) reserves the dot's leading slot, so
 * all labels start at the same x whether or not they have a dot.
 */
export function OptionLabel({
  option,
  reservePresenceSlot = false,
}: {
  option: PersonaDropdownOption;
  reservePresenceSlot?: boolean;
}) {
  if (option.presence || reservePresenceSlot) {
    return (
      <span className="flex min-w-0 items-center gap-2">
        <PresenceSlot option={option} />
        <OptionText option={option} />
      </span>
    );
  }
  return <OptionText option={option} />;
}

/** The leading presence slot: the option's dot, or a same-size blank. */
export function PresenceSlot({
  option,
}: {
  option?: Pick<PersonaDropdownOption, "presence">;
}) {
  return option?.presence ? (
    <PresenceDot status={option.presence} />
  ) : (
    <span
      aria-hidden="true"
      className="inline-flex h-2.5 w-2.5 shrink-0"
      data-testid="presence-slot-spacer"
    />
  );
}

function OptionText({ option }: { option: PersonaDropdownOption }) {
  return (
    <span className="flex min-w-0 flex-col">
      <span className="truncate">{option.label}</span>
      {option.description ? (
        <span className="truncate text-xs text-muted-foreground">
          {option.description}
        </span>
      ) : null}
    </span>
  );
}
