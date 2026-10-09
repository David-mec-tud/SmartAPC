import { useState } from "react";
import { useWorkspace } from "../workspace";
import { Button } from "./ui";

/** Enable belongs to the active runtime, including when editing another project. */
export function ControllerControl({
  disabled = false,
}: {
  disabled?: boolean;
}) {
  const { execution, act } = useWorkspace();
  const [pending, setPending] = useState(false);
  if (execution?.mode !== "runtime") return null;
  const status = execution.snapshot.controller;
  return (
    <Button
      size="small"
      variant={status?.enabled ? "outline" : "default"}
      disabled={disabled || pending || !status || status.external_enable}
      title={
        status?.external_enable
          ? "On/off is owned by ExternalControllerEnable"
          : status?.reason
      }
      onClick={() => {
        setPending(true);
        void act("/api/execution/command", {
          action: status?.enabled ? "disable_control" : "enable_control",
          mode: "runtime",
          project_name: execution.snapshot.project_name,
          execution_id: execution.snapshot.execution_id,
        })
          .catch(() => {})
          .finally(() => setPending(false));
      }}
    >
      {status?.external_enable
        ? "ExternalControllerEnable owns on/off"
        : status?.enabled
          ? "Turn controller off"
          : "Turn controller on"}
    </Button>
  );
}
