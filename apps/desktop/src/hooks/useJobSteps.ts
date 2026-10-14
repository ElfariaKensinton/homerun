import { useState, useEffect, useCallback, useRef } from "react";
import { api } from "../api/commands";
import type { StepInfo, StepsResponse } from "../api/types";

interface UseJobStepsResult {
  steps: StepInfo[];
  stepsDiscovered: number;
  jobName: string | null;
  loading: boolean;
  expandedStep: number | null;
  stepLogs: Record<number, string[]>;
  stepLogErrors: Record<number, string>;
  toggleStep: (stepNumber: number) => void;
}

function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return "Unknown error while fetching step logs";
  }
}

export function useJobSteps(runnerId: string | undefined, isBusy: boolean): UseJobStepsResult {
  const [stepsResponse, setStepsResponse] = useState<StepsResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [expandedStep, setExpandedStep] = useState<number | null>(null);
  const [stepLogs, setStepLogs] = useState<Record<number, string[]>>({});
  const [stepLogErrors, setStepLogErrors] = useState<Record<number, string>>({});
  const logCacheRef = useRef<Record<number, string[]>>({});

  // Poll steps every 1s when busy
  useEffect(() => {
    if (!isBusy || !runnerId) {
      setStepsResponse(null);
      setExpandedStep(null);
      setStepLogs({});
      setStepLogErrors({});
      logCacheRef.current = {};
      setLoading(false);
      return;
    }

    setLoading(true);

    const fetchSteps = async () => {
      try {
        const data = await api.getRunnerSteps(runnerId);
        setStepsResponse(data);
      } catch {
        // Ignore transient errors while the daemon is polling runner state.
      } finally {
        setLoading(false);
      }
    };

    fetchSteps();
    const interval = setInterval(fetchSteps, 1000);
    return () => clearInterval(interval);
  }, [isBusy, runnerId]);

  const currentStep = stepsResponse?.steps.find((step) => step.number === expandedStep);
  const currentStepStatus = currentStep?.status;
  const currentStepName = currentStep?.name;

  // Fetch logs for the expanded step. Re-poll while it runs; retry on completion.
  useEffect(() => {
    if (expandedStep === null || !runnerId) return;

    const isRunning = currentStepStatus === "running";
    const isCached = logCacheRef.current[expandedStep] !== undefined;

    // Skip completed steps already fetched successfully.
    if (isCached && !isRunning) return;

    // Clear a prior error/output while this attempt is in flight.
    setStepLogErrors((previous) => {
      if (!(expandedStep in previous)) return previous;
      const next = { ...previous };
      delete next[expandedStep];
      return next;
    });
    setStepLogs((previous) => {
      if (!(expandedStep in previous)) return previous;
      const next = { ...previous };
      delete next[expandedStep];
      return next;
    });

    const fetchLogs = async () => {
      try {
        const data = await api.getStepLogs(runnerId, expandedStep);
        const lines = data.lines;

        // Cache only after a step is no longer running.
        if (currentStepStatus && currentStepStatus !== "running") {
          logCacheRef.current[expandedStep] = lines;
        }

        setStepLogErrors((previous) => {
          if (!(expandedStep in previous)) return previous;
          const next = { ...previous };
          delete next[expandedStep];
          return next;
        });
        setStepLogs((previous) => ({ ...previous, [expandedStep]: lines }));
      } catch (error) {
        // GitHub may not publish logs until a running step completes. Preserve that
        // expected state, but show failures for completed steps instead of hiding them.
        if (isRunning) {
          setStepLogErrors((previous) => {
            if (!(expandedStep in previous)) return previous;
            const next = { ...previous };
            delete next[expandedStep];
            return next;
          });
        } else {
          setStepLogErrors((previous) => ({
            ...previous,
            [expandedStep]: errorMessage(error),
          }));
        }

        if (logCacheRef.current[expandedStep] === undefined) {
          setStepLogs((previous) => ({ ...previous, [expandedStep]: [] }));
        }
      }
    };

    fetchLogs();

    if (!isRunning) return;
    const interval = setInterval(fetchLogs, 5000);
    return () => clearInterval(interval);
  }, [expandedStep, runnerId, currentStepStatus, currentStepName]);

  const toggleStep = useCallback((stepNumber: number) => {
    setExpandedStep((previous) => (previous === stepNumber ? null : stepNumber));
  }, []);

  return {
    steps: stepsResponse?.steps ?? [],
    stepsDiscovered: stepsResponse?.steps_discovered ?? 0,
    jobName: stepsResponse?.job_name ?? null,
    loading,
    expandedStep,
    stepLogs,
    stepLogErrors,
    toggleStep,
  };
}
