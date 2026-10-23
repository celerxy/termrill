/** 指针捕获限制拖动生命周期；键盘与双击均可调整/恢复，取消拖动不保存。 */
import { useRef } from 'react';
export default function ResizeSeparator({
  axis,
  label,
  value,
  min,
  max,
  initial,
  step,
  pixelsPerUnit = () => 1,
  direction = 1,
  onChange,
  onCommit,
}: {
  axis: 'x' | 'y';
  label: string;
  value: number;
  min: number;
  max: number;
  initial: number;
  step: number;
  pixelsPerUnit?: () => number;
  direction?: number;
  onChange: (value: number) => void;
  onCommit: (value: number) => void;
}) {
  const drag = useRef<{ coordinate: number; value: number; scale: number; latest: number } | null>(
    null,
  );
  const clamp = (next: number) => Math.min(max, Math.max(min, next));
  return (
    <div
      role="separator"
      tabIndex={0}
      className={`workspace-resize workspace-resize-${axis}`}
      aria-label={label}
      aria-orientation={axis === 'x' ? 'vertical' : 'horizontal'}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuenow={Math.round(value)}
      title={`${label}；方向键调整，双击恢复默认`}
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        event.preventDefault();
        event.currentTarget.focus();
        event.currentTarget.setPointerCapture(event.pointerId);
        drag.current = {
          coordinate: axis === 'x' ? event.clientX : event.clientY,
          value,
          scale: Math.max(0.1, pixelsPerUnit()),
          latest: value,
        };
      }}
      onPointerMove={(event) => {
        const start = drag.current;
        if (!start) return;
        start.latest = clamp(
          start.value +
            (direction * ((axis === 'x' ? event.clientX : event.clientY) - start.coordinate)) /
              start.scale,
        );
        onChange(start.latest);
      }}
      onPointerUp={(event) => {
        const start = drag.current;
        if (!start) return;
        drag.current = null;
        event.currentTarget.releasePointerCapture(event.pointerId);
        onCommit(start.latest);
      }}
      onLostPointerCapture={() => {
        const start = drag.current;
        drag.current = null;
        if (start) onChange(start.value);
      }}
      onPointerCancel={() => {
        const start = drag.current;
        drag.current = null;
        if (start) onChange(start.value);
      }}
      onDoubleClick={() => {
        onChange(initial);
        onCommit(initial);
      }}
      onKeyDown={(event) => {
        let next: number;
        if (event.key === 'Home') next = initial;
        else if (event.key === (axis === 'x' ? 'ArrowLeft' : 'ArrowDown')) next = value - step;
        else if (event.key === (axis === 'x' ? 'ArrowRight' : 'ArrowUp')) next = value + step;
        else if (event.key === 'Escape' && drag.current) {
          onChange(drag.current.value);
          drag.current = null;
          event.preventDefault();
          return;
        } else return;
        event.preventDefault();
        onChange(clamp(next));
        onCommit(clamp(next));
      }}
    >
      <span />
    </div>
  );
}
