import { PresenceDot } from "@/features/presence/ui/PresenceBadge";
import type { PersonaDropdownOption } from "./agentConfigOptions";

/** Option label with the harness's optional description as a muted second line. */
export function OptionLabel({ option }: { option: PersonaDropdownOption }) {
  if (option.presence) {
    return (
      <span className="flex min-w-0 items-center gap-2">
        <PresenceDot status={option.presence} />
        <OptionText option={option} />
      </span>
    );
  }
  return <OptionText option={option} />;
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
