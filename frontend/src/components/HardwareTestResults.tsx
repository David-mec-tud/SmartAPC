import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { CircleCheck, CircleX, LoaderCircle } from "lucide-react";
import { type Job, type SignalValue, projectPath, request } from "../api";
import { useWorkspace } from "../workspace";
import { Badge, Notice } from "./ui";

type TestKind = "watchdog_test" | "io_read" | "io_write";
type TestRequest =
  | { kind: "watchdog_test" | "io_read" }
  | { kind: "io_write"; values: Record<string, SignalValue> };
const labels: Record<TestKind, string> = {
  watchdog_test: "Watchdog test",
  io_read: "Read test",
  io_write: "Write test",
};
interface Reading {
  signal: string;
  address: string;
  value?: SignalValue;
  error?: string;
}
interface TestFailure {
  message: string;
  attemptedAt: number;
}
const failureKey = (project: string, kind: string) => [
  "hardware-test-failure",
  project,
  kind,
];
function shortReason(message: string): string {
  const firstLine = message.split("\n")[0];
  return firstLine.length > 200 ? `${firstLine.slice(0, 197)}…` : firstLine;
}

/** Keep submission feedback beside the action while the workspace refreshes jobs. */
export function useHardwareTest() {
  const { project, startJob } = useWorkspace();
  const client = useQueryClient();
  return useMutation({
    mutationFn: (body: TestRequest) => startJob(body),
    onError: (error, body) => {
      // Keep a rejected request visible across navigation and retries as well.
      client.setQueryData<TestFailure>(failureKey(project, body.kind), {
        message: error.message,
        attemptedAt: Date.now(),
      });
    },
  });
}

export function HardwareTestResults({
  kinds,
  test,
}: {
  kinds: TestKind[];
  test: ReturnType<typeof useHardwareTest>;
}) {
  const { project, jobs } = useWorkspace();
  return (
    <div className="hardware-test-results">
      <h3>Test results</h3>
      {jobs.error && <Notice error>{jobs.error.message}</Notice>}
      {kinds.map((kind) => {
        // The backend lists newest first. Keep read and write outcomes separately,
        // and scope both summaries and full reports to the selected project.
        const history = jobs.data?.filter(
          (job) => job.project_name === project && job.kind === kind,
        );
        const running = history?.find((job) => job.phase === "running");
        const completed = history?.find((job) => job.phase !== "running");
        const submitting = test.variables?.kind === kind && test.isPending;
        return (
          <section key={kind} aria-label={`${labels[kind]} result`}>
            {(submitting || running) && (
              <Notice>
                <span className="hardware-test-feedback">
                  <LoaderCircle size={18} aria-hidden="true" />
                  {submitting ? "Starting" : "Running"}{" "}
                  {labels[kind].toLowerCase()}… Waiting for hardware.
                </span>
              </Notice>
            )}
            <TestResult
              key={`${project}-${kind}`}
              summary={completed}
              kind={kind}
            />
            {!completed && !running && !submitting && (
              <p className="muted">
                {jobs.isLoading ? "Loading" : "No"} {labels[kind].toLowerCase()}{" "}
                results yet.
              </p>
            )}
          </section>
        );
      })}
    </div>
  );
}

function TestResult({ summary, kind }: { summary?: Job; kind: TestKind }) {
  const { project } = useWorkspace();
  const client = useQueryClient();
  const failure = useQuery<TestFailure | null>({
    queryKey: failureKey(project, kind),
    queryFn: () => null,
    enabled: false,
    gcTime: Infinity,
  });
  // Job lists intentionally omit results. Fetch the persisted report on completion;
  // including the phase in the key prevents a cached running response hiding it.
  const detail = useQuery({
    queryKey: ["job-detail", project, summary?.id, summary?.phase],
    queryFn: () => request<Job>(`${projectPath(project)}/jobs/${summary!.id}`),
    enabled: !!summary,
    // A retry's compact summary is not proof that all read nodes passed. Keep
    // the last report visible until the new full report has been received.
    placeholderData: (previous) => previous,
  });
  const job = summary?.phase === "failed" ? summary : (detail.data ?? summary);
  const result = job?.result;
  const readings = (result?.readings ?? []) as Reading[];
  const errors = readings.filter((reading) => reading.error).length;
  const firstError = readings.find((reading) => reading.error);
  const currentFailure =
    job?.phase === "failed"
      ? job.error || "The hardware test could not complete."
      : errors
        ? `${errors} of ${readings.length} nodes could not be read. ${firstError?.signal}: ${shortReason(firstError?.error || "")}`
        : null;
  const disabled = result?.status === "disabled";
  const passed =
    job?.phase === "succeeded" &&
    !!result &&
    !errors &&
    !(kind === "io_read" && !readings.length);
  useEffect(() => {
    if (!job) return;
    const attemptedAt = Date.parse(job.started_at);
    // Keep failures through retries, cancellations and empty read tests. An old
    // successful report cannot clear a more recent rejected attempt.
    if (
      currentFailure &&
      (!failure.data || attemptedAt > failure.data.attemptedAt)
    )
      client.setQueryData<TestFailure>(failureKey(project, kind), {
        message: currentFailure,
        attemptedAt,
      });
    else if (passed && failure.data && attemptedAt >= failure.data.attemptedAt)
      client.setQueryData(failureKey(project, kind), null);
  }, [client, project, kind, job, passed, currentFailure, failure.data]);
  const failureMessage = failure.data?.message || currentFailure;
  const status = failureMessage
    ? "Test failed"
    : job?.phase === "cancelled"
      ? "Cancelled"
      : disabled
        ? "Disabled"
        : !result
          ? "Loading result"
          : kind === "io_read" && !readings.length
            ? "No nodes"
            : "Passed";
  if (!job && !failureMessage) return null;
  return (
    <>
      <div className="section-title">
        <div>
          <strong>{labels[kind]}</strong>
          {job && (
            <small className="muted">
              {new Date(job.finished_at ?? job.started_at).toLocaleString()}
            </small>
          )}
        </div>
        <span role="status" aria-live="polite">
          <Badge
            tone={
              status === "Test failed"
                ? "danger"
                : status === "Passed"
                  ? "good"
                  : "neutral"
            }
          >
            {status}
          </Badge>
        </span>
      </div>
      {failureMessage && (
        <Notice error>
          <span className="hardware-test-feedback">
            <CircleX size={18} aria-hidden="true" />
            <span>
              <strong>Test failed.</strong>{" "}
              <span title={failureMessage}>{shortReason(failureMessage)}</span>
            </span>
          </span>
        </Notice>
      )}
      {detail.error && (
        <Notice error>
          Could not load test result: {detail.error.message}
        </Notice>
      )}
      {!failureMessage && result && kind === "watchdog_test" && (
        <Notice>
          <span className="hardware-test-feedback">
            <CircleCheck size={18} aria-hidden="true" />
            <span>
              {disabled ? (
                String(result.message)
              ) : (
                <>
                  Counter {String(result.counter)} read and mirrored
                  successfully. Communication cycle:{" "}
                  {String(result.communication_cycle_ms)} ms; stale timeout:{" "}
                  {String(result.stale_timeout_ms)} ms.
                </>
              )}
            </span>
          </span>
        </Notice>
      )}
      {!failureMessage && result && kind === "io_write" && (
        <Notice>
          <span className="hardware-test-feedback">
            <CircleCheck size={18} aria-hidden="true" />
            <span>
              Successfully wrote {String(result.writes)} test value
              {result.writes === 1 ? "" : "s"} to hardware.
            </span>
          </span>
        </Notice>
      )}
      {result && kind === "io_read" && (
        <>
          {!failureMessage && (
            <Notice error={errors > 0}>
              <span className="hardware-test-feedback">
                <CircleCheck size={18} aria-hidden="true" />
                <span>
                  {readings.length
                    ? `${readings.length} of ${readings.length} nodes read successfully.`
                    : "No configured I/O nodes to read."}
                </span>
              </span>
            </Notice>
          )}
          {readings.length > 0 && (
            <div className="table-scroll">
              <table>
                <thead>
                  <tr>
                    <th>Signal</th>
                    <th>Node ID</th>
                    <th>Value / error</th>
                  </tr>
                </thead>
                <tbody>
                  {readings.map((reading, index) => (
                    <tr key={`${reading.signal}-${index}`}>
                      <th>{reading.signal}</th>
                      <td>{reading.address}</td>
                      <td>
                        {reading.error ? (
                          <span className="error-text">{reading.error}</span>
                        ) : (
                          String(reading.value?.value ?? "—")
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </>
  );
}
