import { Download, Users } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import type { AppId } from "@/lib/api/types";

interface ProviderEmptyStateProps {
  appId: AppId;
  onCreate?: () => void;
  onImport?: () => void;
}

export function ProviderEmptyState({
  appId,
  onCreate,
  onImport,
}: ProviderEmptyStateProps) {
  const { t } = useTranslation();
  const isPi = appId === "pi";
  const showSnippetHint =
    appId === "claude" || appId === "codex" || appId === "gemini";

  return (
    <div className="empty-state">
      <div className="mb-5 flex h-16 w-16 items-center justify-center rounded-2xl bg-muted/80 ring-1 ring-border/70">
        <Users className="h-7 w-7 text-muted-foreground" />
      </div>
      <h3 className="page-title">
        {isPi ? t("pi.empty.title") : t("provider.noProviders")}
      </h3>
      <p className="page-meta mt-2 max-w-lg">
        {isPi
          ? t("pi.empty.description")
          : t("provider.noProvidersDescription")}
      </p>
      {showSnippetHint && (
        <p className="page-meta mt-1.5 max-w-lg">
          {t("provider.noProvidersDescriptionSnippet")}
        </p>
      )}
      <div className="mt-6 flex flex-col gap-2">
        {onImport && (
          <Button onClick={onImport}>
            <Download className="mr-2 h-4 w-4" />
            {appId === "claude-desktop"
              ? t("provider.importFromClaude", {
                  defaultValue: "将 Claude Code 中已有的供应商导入",
                })
              : t("provider.importCurrent")}
          </Button>
        )}
        {onCreate && (
          <Button variant={onImport ? "outline" : "default"} onClick={onCreate}>
            {t("provider.addProvider")}
          </Button>
        )}
      </div>
    </div>
  );
}
