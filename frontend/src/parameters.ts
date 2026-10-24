/** Friendly presentation keeps the exact backend key visible and authoritative. */
export function parameterLabel(key: string) {
  const field = key.split(".").at(-1) ?? key;
  const labels: Record<string, string> = {
    prediction_horizon: "Prediction horizon",
    control_horizon: "Control horizon",
    band_low: "Band low offset",
    band_high: "Band high offset",
    use_l1: "MV movement cost",
    max_delta_u: "Maximum MV movement",
    optimization_mode_LL: "Optimization safety low",
    optimization_mode_HL: "Optimization safety high",
    initial_p: "Initial covariance",
    bias_q: "Output bias process noise",
    kp: "Proportional gain",
    bias: "Controller bias",
  };
  return (
    labels[field] ??
    (field === "q"
      ? key.includes("kalman")
        ? "Process noise covariance"
        : "CV tracking weight"
      : field === "r"
        ? key.includes("kalman")
          ? "Measurement noise covariance"
          : "MV movement weight"
        : field.replaceAll("_", " "))
  );
}
export function parameterGroup(key: string) {
  if (/^io\.cv\..*\.optimization_mode_(LL|HL)$/.test(key))
    return "Optimization safety";
  if (key.startsWith("mpc.kalman.")) return "Observer";
  if (key.startsWith("io.")) return "Static I/O";
  const variable = /^mpc\.(cv|mv)\.([^.]+)\./.exec(key);
  if (variable) return `${variable[1].toUpperCase()} · ${variable[2]}`;
  if (key.startsWith("mpc.") || key.startsWith("p."))
    return "Controller settings";
  return "Custom parameters";
}
export function groupParameters(
  parameters: Record<string, number | boolean>,
  search: string,
) {
  const groups = new Map<string, [string, number | boolean][]>();
  const query = search.trim().toLowerCase();
  for (const [key, value] of Object.entries(parameters).sort(([a], [b]) =>
    a.localeCompare(b),
  )) {
    const group = parameterGroup(key);
    if (!`${key} ${parameterLabel(key)} ${group}`.toLowerCase().includes(query))
      continue;
    groups.set(group, [...(groups.get(group) ?? []), [key, value]]);
  }
  const order = [
    "Controller settings",
    "CV",
    "MV",
    "Observer",
    "Static I/O",
    "Optimization safety",
    "Custom parameters",
  ];
  const rank = (group: string) =>
    order.findIndex((prefix) => group.startsWith(prefix));
  return [...groups.entries()].sort(
    ([a], [b]) => rank(a) - rank(b) || a.localeCompare(b),
  );
}
