import { useEffect, useRef, useState } from "react";
import { download, projectPath, request, type ProjectConfig } from "../api";
import { Button, Notice } from "./ui";

/** A complete, validated file replaces the I/O draft in one edit. Rust owns
 * the CSV format, variable names, Node IDs, types and limit validation. */
export function IoCsv({
  project,
  config,
  disabled,
  onChange,
}: {
  project: string;
  config: ProjectConfig;
  disabled: boolean;
  onChange: (draft: Pick<ProjectConfig, "variables" | "io_definition">) => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const mounted = useRef(true);
  const fingerprint = JSON.stringify(config);
  const latest = useRef({ fingerprint, disabled });
  latest.current = { fingerprint, disabled };
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; error?: boolean }>();
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  async function transfer(file?: File) {
    setBusy(true);
    setMessage(undefined);
    try {
      const path = `${projectPath(project)}/io`;
      const result = file
        ? await request<Pick<ProjectConfig, "variables" | "io_definition">>(
            `${path}/import`,
            { csv: await file.text() },
            "POST",
          )
        : await request<{ csv: string }>(`${path}/example`);
      if (!mounted.current) return;
      // A late response must not overwrite newer edits or a different project.
      if (latest.current.disabled || latest.current.fingerprint !== fingerprint)
        throw Error(
          "Configuration changed during the CSV request. Please try again.",
        );
      if ("csv" in result) {
        download(
          `${project}-io-example.csv`,
          result.csv,
          "text/csv;charset=utf-8",
        );
      } else {
        onChange(result);
        setMessage({
          text: "I/O definitions imported into the draft. Save configuration to keep them.",
        });
      }
    } catch (error) {
      if (mounted.current)
        setMessage({ text: (error as Error).message, error: true });
    } finally {
      if (mounted.current) setBusy(false);
    }
  }

  return (
    <section className="panel">
      <h2>I/O CSV</h2>
      <p className="muted">
        Download an example with MV, CV, DV and custom variables. Use one row
        per variable and replace the example OPC UA Node IDs with your own.
        Import replaces all four I/O sections in this draft. Save configuration
        to persist the definitions.
      </p>
      <div className="inline">
        <Button
          variant="outline"
          disabled={disabled || busy}
          onClick={() => void transfer()}
        >
          Download I/O CSV example
        </Button>
        <Button
          variant="outline"
          disabled={disabled || busy}
          onClick={() => input.current?.click()}
        >
          {busy ? "Processing CSV…" : "Import I/O CSV"}
        </Button>
        <input
          ref={input}
          type="file"
          aria-label="Import I/O CSV"
          accept=".csv"
          hidden
          disabled={disabled || busy}
          onChange={(event) => {
            const file = event.target.files?.[0];
            event.target.value = "";
            if (file) void transfer(file);
          }}
        />
      </div>
      {message && <Notice error={message.error}>{message.text}</Notice>}
    </section>
  );
}
