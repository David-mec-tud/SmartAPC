// Shared shadcn/ui primitives, kept locally so product styles and accessibility
// remain reviewable. Radix Slot supports links with the same button treatment.
import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";
export const cn = (...values: ClassValue[]) => twMerge(clsx(values));
const variants = cva("button", {
  variants: {
    variant: {
      default: "button-primary",
      outline: "button-outline",
      ghost: "button-ghost",
      destructive: "button-danger",
    },
    size: { default: "", small: "button-small" },
  },
  defaultVariants: { variant: "default", size: "default" },
});
export function Button({
  className,
  variant,
  size,
  asChild,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof variants> & { asChild?: boolean }) {
  const Component = asChild ? Slot : "button";
  return (
    <Component
      className={cn(variants({ variant, size }), className)}
      {...props}
    />
  );
}
export function Input({ className, ...props }: React.ComponentProps<"input">) {
  return <input className={cn("input", className)} {...props} />;
}
export function Select({
  className,
  ...props
}: React.ComponentProps<"select">) {
  return <select className={cn("input", className)} {...props} />;
}
export function Badge({
  children,
  tone = "neutral",
}: {
  children: React.ReactNode;
  tone?: string;
}) {
  return <span className={`badge badge-${tone}`}>{children}</span>;
}
export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="field">
      <span>{label}</span>
      {children}
      {hint && <small>{hint}</small>}
    </label>
  );
}
export function Notice({
  children,
  error,
}: {
  children: React.ReactNode;
  error?: boolean;
}) {
  return (
    <div
      className={`notice ${error ? "notice-error" : ""}`}
      role={error ? "alert" : "status"}
    >
      {children}
    </div>
  );
}
/** Text editing retains blank/intermediate numeric drafts; the server validates
 * final values. Empty optional fields stay null instead of becoming numeric zero. */
export function NumberInput({
  value,
  onChange,
  optional,
  ...props
}: Omit<React.ComponentProps<typeof Input>, "value" | "onChange"> & {
  value: number | null | undefined;
  onChange: (value: number | null) => void;
  optional?: boolean;
}) {
  const [text, setText] = React.useState(value == null ? "" : String(value));
  React.useEffect(() => setText(value == null ? "" : String(value)), [value]);
  return (
    <Input
      {...props}
      inputMode="decimal"
      value={text}
      onChange={(event) => {
        const next = event.target.value;
        setText(next);
        if (next === "" && optional) onChange(null);
        else if (next.trim() && Number.isFinite(Number(next)))
          onChange(Number(next));
      }}
      onBlur={(event) => {
        if (!text.trim() && !optional) setText(String(value ?? 0));
        props.onBlur?.(event);
      }}
    />
  );
}
