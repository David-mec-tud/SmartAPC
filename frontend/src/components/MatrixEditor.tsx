import { useId } from "react";
import { type Matrix } from "../api";
import { Button, NumberInput } from "./ui";
/** Preserve row/column identities when resizing and never flatten matrix ordering. */
export function resizeMatrix(
  matrix: Matrix,
  rows: number,
  cols: number,
  fill = 0,
): Matrix {
  return Array.from({ length: rows }, (_, row) =>
    Array.from({ length: cols }, (_, col) => matrix[row]?.[col] ?? fill),
  );
}
export function MatrixEditor({
  name,
  matrix,
  rows,
  cols,
  onChange,
  disabled,
}: {
  name: string;
  matrix: Matrix;
  rows?: string[];
  cols?: string[];
  onChange: (matrix: Matrix) => void;
  disabled?: boolean;
}) {
  const id = useId();
  async function importMatrix(file: File) {
    const text = await file.text();
    const values = text
      .trim()
      .split(/\r?\n/)
      .filter((line) => line.trim())
      .map((line) =>
        line
          .trim()
          .split(/[\s,;]+/)
          .map(Number),
      );
    if (
      values.length !== matrix.length ||
      values.some(
        (row) =>
          row.length !== (matrix[0]?.length ?? 0) ||
          row.some((value) => !Number.isFinite(value)),
      )
    )
      throw Error(
        `Expected ${matrix.length} × ${matrix[0]?.length ?? 0} finite values`,
      );
    onChange(values);
  }
  return (
    <section className="matrix-block">
      <div className="section-title">
        <strong>{name}</strong>
        <label className="upload-button" htmlFor={id}>
          Import CSV / text
        </label>
        <input
          id={id}
          type="file"
          accept=".csv,.txt"
          hidden
          disabled={disabled}
          onChange={(event) => {
            const file = event.target.files?.[0];
            if (file)
              void importMatrix(file).catch((error) =>
                alert((error as Error).message),
              );
            event.target.value = "";
          }}
        />
      </div>
      <div className="table-scroll">
        <table>
          <thead>
            <tr>
              <th>State / signal</th>
              {matrix[0]?.map((_, col) => (
                <th key={col}>{cols?.[col] ?? `x${col + 1}`}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {matrix.map((row, r) => (
              <tr key={r}>
                <th>{rows?.[r] ?? `x${r + 1}`}</th>
                {row.map((value, col) => (
                  <td key={col}>
                    <NumberInput
                      aria-label={`${name} row ${r + 1} column ${col + 1}`}
                      value={value}
                      disabled={disabled}
                      onChange={(next) => {
                        const values = structuredClone(matrix);
                        values[r][col] = next ?? 0;
                        onChange(values);
                      }}
                    />
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
