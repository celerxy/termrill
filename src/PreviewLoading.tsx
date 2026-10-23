/** 真实请求阶段与单调时钟计时；动画只表示工作仍在处理，不推算完成百分比。 */
import { useEffect, useState } from 'react';
import { Archive, FileCode2 } from 'lucide-react';
import './previewLoading.css';
export default function PreviewLoading({
  title,
  detail,
  archive = false,
  editor = false,
}: {
  title: string;
  detail?: string;
  archive?: boolean;
  editor?: boolean;
}) {
  const [seconds, setSeconds] = useState(0);
  useEffect(() => {
    const started = performance.now();
    const timer = window.setInterval(
      () => setSeconds(Math.floor((performance.now() - started) / 1000)),
      250,
    );
    return () => window.clearInterval(timer);
  }, []);
  return (
    <div
      className={`preview-loading${editor ? ' preview-loading-editor' : ''}`}
      role="status"
      aria-live="polite"
      aria-busy="true"
    >
      {editor && (
        <div className="preview-editor-skeleton top" aria-hidden="true">
          {[1, 2, 3, 4, 5, 6].map((line) => (
            <div key={line}>
              <span>{line}</span>
              <i />
            </div>
          ))}
        </div>
      )}
      <div className="preview-loading-ring">
        {archive ? <Archive size={36} /> : <FileCode2 size={36} />}
      </div>
      <h3>{title}</h3>
      <p>{detail || '正在准备内容，请稍候'}</p>
      <div className="preview-loading-track" aria-hidden="true">
        <span />
      </div>
      <small aria-live="off">
        已用时间 {String(Math.floor(seconds / 60)).padStart(2, '0')}:
        {String(seconds % 60).padStart(2, '0')}
      </small>
      <small>文件较大或网络较慢时，读取可能需要更长时间</small>
      <div
        className={editor ? 'preview-editor-skeleton bottom' : 'preview-loading-skeleton'}
        aria-hidden="true"
      >
        <i />
        <i />
        <i />
      </div>
    </div>
  );
}
