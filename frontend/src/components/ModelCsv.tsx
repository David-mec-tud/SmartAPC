import { useEffect, useRef, useState } from "react";
import {
  download,
  projectPath,
  request,
  type Model,
  type ProjectConfig,
} from "../api";
import { Button, Notice } from "./ui";

/** A complete, validated file replaces the model draft in one edit. Rust owns
 * the CSV format, signal ordering and numeric/shape validation. */
export function ModelCsv({
  project,
  config,
  disabled,
  onChange,
}: {
  project: string;
  config: ProjectConfig;
  disabled: boolean;
  onChange: (model: Model) => void;
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
      const layout = {
        state_count: config.model?.state_count ?? 1,
        variables: config.variables,
      };
      const path = `${projectPath(project)}/model`;
      const result = file
        ? await request<Model>(
            `${path}/import`,
            { ...layout, csv: await file.text() },
            "POST",
          )
        : await request<{ csv: string }>(`${path}/template`, layout, "POST");
      if (!mounted.current) return;
      // A late response must not overwrite newer edits or a different project.
      if (latest.current.disabled || latest.current.fingerprint !== fingerprint)
        throw Error(
          "Configuration changed during the CSV request. Please try again.",
        );
      if ("csv" in result) {
        download(
          `${project}-model-template.csv`,
          result.csv,
          "text/csv;charset=utf-8",
        );
      } else {
        onChange(result);
        setMessage({
          text: "All four matrices imported into the draft. Save configuration to keep them.",
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
      <h2>Model CSV</h2>
      <p className="muted">
        Download one template for A, B, C and D using the current MV, CV, DV
        names and state dimensions. Replace a11, a12, b11 and the other
        placeholders with numeric coefficients. Keep the matrix headings and
        row/column labels.
      </p>
      <div className="inline">
        <Button
          variant="outline"
          disabled={disabled || busy}
          onClick={() => void transfer()}
        >
          Download model CSV template
        </Button>
        <Button
          variant="outline"
          disabled={disabled || busy}
          onClick={() => input.current?.click()}
        >
          {busy ? "Processing CSV…" : "Import model CSV"}
        </Button>
        <input
          ref={input}
          type="file"
          aria-label="Import model CSV"
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
