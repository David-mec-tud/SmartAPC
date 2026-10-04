import { useEffect, useRef } from "react";
import type * as PlotlyTypes from "plotly.js";
type Plotly = typeof PlotlyTypes;
declare global {
  interface Window {
    Plotly: Plotly;
  }
}
let bundle: Promise<Plotly> | undefined;
function loadPlotly() {
  if (!bundle)
    bundle = new Promise<Plotly>((resolve, reject) => {
      if (window.Plotly) {
        resolve(window.Plotly);
        return;
      }
      const script = document.createElement("script");
      script.src = "/assets/plotly-2.35.2.min.js";
      script.onload = () => resolve(window.Plotly);
      script.onerror = () => {
        bundle = undefined;
        reject(Error("Local Plotly bundle could not be loaded"));
      };
      document.head.appendChild(script);
    });
  return bundle;
}
export function Plot({
  data,
  title,
  revision = title,
  height = 320,
}: {
  data: PlotlyTypes.Data[];
  title: string;
  revision?: string;
  height?: number;
}) {
  const host = useRef<HTMLDivElement>(null);
  // Purge only when the component leaves the page. Purging on each data update
  // would discard Plotly's zoom and legend state despite a stable uirevision.
  useEffect(() => {
    const element = host.current;
    return () => {
      if (element && window.Plotly) window.Plotly.purge(element);
    };
  }, []);
  useEffect(() => {
    const element = host.current;
    if (!element) return;
    let disposed = false;
    let observer: ResizeObserver | undefined;
    let theme: MutationObserver | undefined;
    void loadPlotly()
      .then((plotly) => {
        if (disposed) return;
        const draw = () => {
          const style = getComputedStyle(document.documentElement);
          void plotly.react(
            element,
            data,
            {
              title: { text: title, font: { size: 14 } },
              height,
              paper_bgcolor: style.getPropertyValue("--surface").trim(),
              plot_bgcolor: style.getPropertyValue("--surface").trim(),
              font: {
                family: "system-ui, sans-serif",
                color: style.getPropertyValue("--text").trim(),
                size: 11,
              },
              colorway: [
                "#2c8a7a",
                "#527ad4",
                "#b17a34",
                "#b86d9b",
                "#6d8a46",
                "#aa6458",
              ],
              xaxis: {
                gridcolor: style.getPropertyValue("--border").trim(),
                automargin: true,
              },
              yaxis: {
                gridcolor: style.getPropertyValue("--border").trim(),
                automargin: true,
              },
              margin: { t: 44, r: 20, b: 40, l: 54 },
              legend: { orientation: "h", y: -0.2 },
              uirevision: revision,
              hovermode: "x unified",
            },
            { responsive: true, displaylogo: false, scrollZoom: true },
          );
        };
        draw();
        observer = new ResizeObserver(() => void plotly.Plots.resize(element));
        observer.observe(element);
        theme = new MutationObserver(draw);
        theme.observe(document.documentElement, {
          attributes: true,
          attributeFilter: ["class"],
        });
      })
      .catch((error) => {
        if (!disposed) element.textContent = (error as Error).message;
      });
    return () => {
      disposed = true;
      observer?.disconnect();
      theme?.disconnect();
    };
  }, [data, title, revision, height]);
  return <div className="plot" ref={host} aria-label={title} />;
}
