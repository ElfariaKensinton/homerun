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

  // Switching runners must not carry the previous runner's expanded step/logs over.
  useEffect(() => {
    setStepsResponse(null);
    setExpandedStep(null);
    setStepLogs({});
    setStepLogErrors({});
    logCacheRef.current = {};
  }, [runnerId]);

  // Fetch once while idle so the most recent completed job remains inspectable.
  // Poll more frequently while a job is active.
  useEffect(() => {
    if (!runnerId) {
      setStepsResponse(null);
      setLoading(false);
      return;
    }

    const fetchSteps = async () => {
      try {
        const data = await api.getRunnerSteps(runnerId);
        setStepsResponse(data);
      } catch {
        // Ignore transient errors during daemon startup/state transitions.
      } finally {
        setLoading(false);
      }
    };

    setLoading(true);
    fetchSteps();
    if (!isBusy) return;

    const interval = setInterval(fetchSteps, 1000);
    return () => clearInterval(interval);
  }, [isBusy, runnerId]);

  const currentStep = stepsResponse?.steps.find((step) => step.number === expandedStep);
  const currentStepStatus = currentStep?.status;
  const currentStepName = currentStep?.name;

  // Fetch live local Worker output while busy. Keep retrying through transient
  // GitHub 404s, then perform a final fetch after the job completes.
  useEffect(() => {
    if (expandedStep === null || !runnerId) return;

    const isRunning = currentStepStatus === "running";
    const isCached = logCacheRef.current[expandedStep] !== undefined;
    if (isCached && !isRunning) return;

    const clearError = () => {
      setStepLogErrors((previous) => {
        if (!(expandedStep in previous)) return previous;
        const next = { ...previous };
        delete next[expandedStep];
        return next;
      });
    };

    const fetchLogs = async () => {
      try {
        const data = await api.getStepLogs(runnerId, expandedStep);
        const lines = data.lines;

        if (lines.length === 0 && isBusy) {
          setStepLogErrors((previous) => ({
            ...previous,
            [expandedStep]: "Waiting for live step output… Retrying...",
          }));
          // Keep the last non-empty response on screen and do not cache an empty poll.
          return;
        }

        clearError();
        setStepLogs((previous) => ({ ...previous, [expandedStep]: lines }));
        if (currentStepStatus && currentStepStatus !== "running" && lines.length > 0) {
          logCacheRef.current[expandedStep] = lines;
        }
      } catch (error) {
        const message = errorMessage(error);
        const logsNotReady = message.includes("HTTP 404") || message.includes("BlobNotFound");

        if (isBusy && logsNotReady) {
          setStepLogErrors((previous) => ({
            ...previous,
            [expandedStep]: "GitHub has not published this step's logs yet. Retrying...",
          }));
        } else {
          setStepLogErrors((previous) => ({
            ...previous,
            [expandedStep]: message,
          }));
        }
      }
    };

    fetchLogs();
    if (!isBusy) return;

    const interval = setInterval(fetchLogs, 2000);
    return () => clearInterval(interval);
  }, [expandedStep, runnerId, currentStepStatus, currentStepName, isBusy]);

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
