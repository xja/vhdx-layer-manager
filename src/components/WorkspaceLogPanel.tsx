import { WorkspaceLogEntry, WorkspaceLogLevel } from "../types";
import { Badge } from "./ui/Badge";
import { Button } from "./ui/Button";
import { Card } from "./ui/Card";

type Props = {
  logs: WorkspaceLogEntry[];
  filter: "all" | "error" | "op";
  onFilterChange: (filter: "all" | "error" | "op") => void;
  onClear: () => void;
  onFocusNode?: (nodeId: string) => void;
  t: (key: string, options?: any) => string;
};

function toneFor(level: WorkspaceLogLevel) {
  switch (level) {
    case "success":
      return "positive" as const;
    case "warn":
      return "warn" as const;
    case "error":
      return "danger" as const;
    default:
      return "neutral" as const;
  }
}

export function WorkspaceLogPanel({
  logs,
  filter,
  onFilterChange,
  onClear,
  onFocusNode,
  t,
}: Props) {
  const filtered = logs.filter((log) => {
    if (filter === "error") return log.level === "error";
    if (filter === "op") return log.source === "op" || log.source === "ui";
    return true;
  });

  return (
    <Card className="flex h-full min-h-0 flex-col overflow-hidden p-4">
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <div>
          <h3 className="text-sm font-semibold text-ink-900">{t("log-panel-title")}</h3>
          <p className="text-xs text-ink-700">{t("log-panel-tip")}</p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {(["all", "op", "error"] as const).map((item) => (
            <Button
              key={item}
              variant={filter === item ? "primary" : "secondary"}
              className="px-2 py-1 text-xs"
              onClick={() => onFilterChange(item)}
            >
              {t(`log-filter-${item}`)}
            </Button>
          ))}
          <Button variant="secondary" className="px-2 py-1 text-xs" onClick={onClear}>
            {t("log-clear")}
          </Button>
        </div>
      </div>

      <div className="min-h-0 flex-1 space-y-2 overflow-y-auto pr-1">
        {filtered.length === 0 ? (
          <div className="rounded-xl border border-dashed border-peach-300/60 bg-white/70 px-4 py-6 text-center text-sm text-ink-700 shadow-inner shadow-peach-300/20">
            {t("log-empty")}
          </div>
        ) : (
          filtered.map((log) => (
            <div
              key={log.id}
              className="rounded-xl border border-peach-200/70 bg-white/80 px-3 py-2 shadow-sm shadow-peach-200/20"
            >
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0 space-y-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <Badge tone={toneFor(log.level)} className="px-2 py-0.5 text-[11px]">
                      {t(`log-level.${log.level}`)}
                    </Badge>
                    <span className="text-[11px] font-mono text-ink-700">
                      {new Date(log.ts).toLocaleTimeString()}
                    </span>
                    {log.command ? (
                      <span className="text-[11px] font-mono text-ink-700">{log.command}</span>
                    ) : null}
                  </div>
                  <p className="text-sm font-semibold text-ink-900">{log.title}</p>
                  {log.detail ? (
                    <pre className="whitespace-pre-wrap break-words font-mono text-[11px] leading-relaxed text-ink-700">
                      {log.detail}
                    </pre>
                  ) : null}
                </div>
                {log.nodeId && onFocusNode ? (
                  <Button
                    variant="secondary"
                    className="px-2 py-1 text-xs"
                    onClick={() => onFocusNode(log.nodeId!)}
                  >
                    {t("log-focus-node")}
                  </Button>
                ) : null}
              </div>
            </div>
          ))
        )}
      </div>
    </Card>
  );
}
