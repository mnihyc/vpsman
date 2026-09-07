import { useEffect, useId, useRef, useState, type PointerEvent } from "react";

export type HsvColor = { h: number; s: number; v: number };

export function hexToHsv(color: string): HsvColor {
  const [r, g, b] = [1, 3, 5].map(
    (offset) => parseInt(color.slice(offset, offset + 2), 16) / 255,
  );
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const delta = max - min;
  let h = 0;
  if (delta !== 0) {
    if (max === r) h = ((g - b) / delta) % 6;
    else if (max === g) h = (b - r) / delta + 2;
    else h = (r - g) / delta + 4;
    h = (h * 60 + 360) % 360;
  }
  return { h, s: max === 0 ? 0 : delta / max, v: max };
}

export function hsvToHex({ h, s, v }: HsvColor): string {
  const sector = (((h % 360) + 360) % 360) / 60;
  const chroma = v * s;
  const x = chroma * (1 - Math.abs((sector % 2) - 1));
  const channels = [
    [chroma, x, 0],
    [x, chroma, 0],
    [0, chroma, x],
    [0, x, chroma],
    [x, 0, chroma],
    [chroma, 0, x],
  ][Math.floor(sector)];
  return `#${channels
    .map((channel) =>
      Math.round((channel + v - chroma) * 255)
        .toString(16)
        .padStart(2, "0"),
    )
    .join("")}`.toUpperCase();
}

export function PingColorWheel({
  color,
  onChange,
}: {
  color: string;
  onChange: (color: string) => void;
}) {
  const [hsv, setHsv] = useState(() => hexToHsv(color));
  const hueInput = useRef<HTMLInputElement>(null);
  const hintId = useId();

  useEffect(() => {
    setHsv((current) => {
      if (hsvToHex(current) === color.toUpperCase()) return current;
      const next = hexToHsv(color);
      // Gray has no hue and black has no saturation. Keep their last chosen
      // coordinates so raising brightness or saturation remains predictable.
      return {
        h: next.s === 0 ? current.h : next.h,
        s: next.v === 0 ? current.s : next.s,
        v: next.v,
      };
    });
  }, [color]);

  function change(next: HsvColor) {
    setHsv(next);
    onChange(hsvToHex(next));
  }

  function point(event: PointerEvent<HTMLDivElement>) {
    const bounds = event.currentTarget.getBoundingClientRect();
    const x =
      (event.clientX - bounds.left - bounds.width / 2) / (bounds.width / 2);
    const y =
      (event.clientY - bounds.top - bounds.height / 2) / (bounds.height / 2);
    const s = Math.min(1, Math.hypot(x, y));
    change({
      ...hsv,
      h:
        s === 0 ? hsv.h : ((Math.atan2(y, x) * 180) / Math.PI + 360) % 360,
      s,
    });
  }

  const angle = (hsv.h * Math.PI) / 180;
  return (
    <>
      <div
        className="pingColorWheel"
        role="group"
        aria-label="Color wheel"
        aria-describedby={hintId}
        onPointerDown={(event) => {
          if (event.button !== 0 || !event.isPrimary) return;
          event.preventDefault();
          hueInput.current?.focus({ preventScroll: true });
          event.currentTarget.setPointerCapture(event.pointerId);
          point(event);
        }}
        onPointerMove={(event) => {
          if (event.currentTarget.hasPointerCapture(event.pointerId))
            point(event);
        }}
        onPointerUp={(event) => {
          if (event.currentTarget.hasPointerCapture(event.pointerId)) {
            point(event);
            event.currentTarget.releasePointerCapture(event.pointerId);
          }
        }}
      >
        <span
          className="pingColorWheelShade"
          aria-hidden="true"
          style={{ opacity: 1 - hsv.v }}
        />
        <span
          className="pingColorWheelThumb"
          aria-hidden="true"
          style={{
            left: `${50 + Math.cos(angle) * hsv.s * 50}%`,
            top: `${50 + Math.sin(angle) * hsv.s * 50}%`,
            backgroundColor: hsvToHex(hsv),
          }}
        />
        <label className="srOnly">
          Color hue
          <input
            ref={hueInput}
            type="range"
            min={0}
            max={360}
            step={1}
            value={hsv.h}
            aria-valuetext={`${Math.round(hsv.h)} degrees`}
            aria-describedby={hintId}
            onChange={(event) =>
              change({ ...hsv, h: event.currentTarget.valueAsNumber })
            }
          />
        </label>
        <label className="srOnly">
          Color saturation
          <input
            type="range"
            min={0}
            max={100}
            step={1}
            value={hsv.s * 100}
            aria-valuetext={`${Math.round(hsv.s * 100)} percent`}
            onChange={(event) =>
              change({ ...hsv, s: event.currentTarget.valueAsNumber / 100 })
            }
          />
        </label>
      </div>
      <label className="pingColorBrightness">
        <span>Brightness</span>
        <input
          className="pingColorBrightnessRange"
          type="range"
          min={0}
          max={255}
          step={1}
          value={hsv.v * 255}
          aria-valuetext={`${Math.round(hsv.v * 100)} percent`}
          style={{
            background: `linear-gradient(to right, #000, ${hsvToHex({ ...hsv, v: 1 })})`,
          }}
          onChange={(event) =>
            change({ ...hsv, v: event.currentTarget.valueAsNumber / 255 })
          }
        />
      </label>
      <span className="pingColorHint" id={hintId}>
        Drag, or use Tab and arrow keys.
      </span>
    </>
  );
}
