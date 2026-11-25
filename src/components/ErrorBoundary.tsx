import React from "react";

interface ErrorBoundaryState {
  error: Error | null;
  componentStack: string | null;
  copied: boolean;
}

export class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  ErrorBoundaryState
> {
  constructor(props: { children: React.ReactNode }) {
    super(props);
    this.state = {
      error: null,
      componentStack: null,
      copied: false,
    };
  }

  static getDerivedStateFromError(error: Error): Partial<ErrorBoundaryState> {
    return { error, copied: false };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error("Splits Coach render error:", error, info.componentStack);
    this.setState({
      error,
      componentStack: info.componentStack ?? null,
    });
  }

  private buildDiagnosticText(): string {
    const { error, componentStack } = this.state;
    const message = error?.message || String(error || "Unknown render error");
    const parts = [`Error: ${message}`];
    if (error?.stack) {
      parts.push(`\nStack:\n${error.stack}`);
    }
    if (componentStack) {
      parts.push(`\nComponent Stack:${componentStack}`);
    }
    return parts.join("\n");
  }

  private handleCopy = async () => {
    const text = this.buildDiagnosticText();
    try {
      await navigator.clipboard.writeText(text);
      this.setState({ copied: true });
      setTimeout(() => this.setState({ copied: false }), 2500);
    } catch {
      try {
        const ta = document.createElement("textarea");
        ta.value = text;
        ta.style.position = "fixed";
        ta.style.opacity = "0";
        document.body.appendChild(ta);
        ta.select();
        document.execCommand("copy");
        document.body.removeChild(ta);
        this.setState({ copied: true });
        setTimeout(() => this.setState({ copied: false }), 2500);
      } catch {
        // Clipboard unavailable in restricted WebView context
      }
    }
  };

  render() {
    const { error, componentStack, copied } = this.state;
    if (!error) {
      return this.props.children;
    }

    const message = error.message || String(error);
    const details = [error.stack, componentStack ? `Component stack:${componentStack}` : null]
      .filter(Boolean)
      .join("\n\n");

    return (
      <div
        style={{
          minHeight: "100vh",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          padding: 24,
          background: "var(--bg, #121614)",
          color: "var(--text, #ecf2ee)",
          fontFamily: '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif',
        }}
      >
        <div
          style={{
            width: "100%",
            maxWidth: 640,
            background: "var(--card, #181e1b)",
            border: "1px solid var(--border, #26312b)",
            borderRadius: 14,
            padding: 24,
            boxShadow: "0 18px 48px rgba(0, 0, 0, 0.28)",
          }}
        >
          <div
            style={{
              fontSize: 12,
              fontWeight: 700,
              textTransform: "uppercase",
              letterSpacing: "0.06em",
              color: "var(--red, #e06c75)",
              marginBottom: 6,
            }}
          >
            Frontend Render Error
          </div>
          <h2 style={{ margin: "0 0 8px", fontSize: 20 }}>
            Splits Coach encountered a render error
          </h2>
          <p
            style={{
              margin: "0 0 14px",
              fontSize: 14,
              lineHeight: 1.5,
              color: "var(--text, #ecf2ee)",
              wordBreak: "break-word",
            }}
          >
            {message}
          </p>
          {details && (
            <pre
              style={{
                margin: "0 0 16px",
                padding: 12,
                maxHeight: 240,
                overflow: "auto",
                borderRadius: 8,
                background: "var(--card-subtle, #1d2521)",
                border: "1px solid var(--border, #26312b)",
                color: "var(--muted, #93a39a)",
                fontSize: 12,
                lineHeight: 1.45,
                whiteSpace: "pre-wrap",
                wordBreak: "break-word",
              }}
            >
              {details}
            </pre>
          )}
          <div style={{ display: "flex", gap: 10, flexWrap: "wrap" }}>
            <button
              type="button"
              onClick={() => void this.handleCopy()}
              style={{
                padding: "8px 14px",
                borderRadius: 8,
                border: "1px solid var(--border, #26312b)",
                background: "transparent",
                color: "var(--text, #ecf2ee)",
                cursor: "pointer",
                fontWeight: 600,
              }}
            >
              {copied ? "Copied" : "Copy Error"}
            </button>
            <button
              type="button"
              onClick={() => window.location.reload()}
              style={{
                padding: "8px 14px",
                borderRadius: 8,
                border: "1px solid transparent",
                background: "var(--accent-primary, #52b788)",
                color: "var(--accent-on-primary, #0b1410)",
                cursor: "pointer",
                fontWeight: 600,
              }}
            >
              Reload App
            </button>
          </div>
        </div>
      </div>
    );
  }
}
