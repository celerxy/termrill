/** 统一错误展示；恢复按钮由调用方提供，诊断摘要必须先展开预览再复制。 */
import { useEffect, useState } from 'react';
import { AlertCircle, Copy } from 'lucide-react';
import { diagnoseError, diagnosticReport } from './errorDiagnosis';
import './errorNotice.css';

type Props = {
  error: string;
  scope: '连接' | '传输' | '压缩包' | '文件编辑' | '文件预览';
  phase?: string;
  disabled?: boolean;
  recovery?: { label: string; run: () => void };
};
export default function ErrorNotice({ error, scope, phase = '失败', disabled, recovery }: Props) {
  const diagnosis = diagnoseError(error);
  const [report, setReport] = useState('');
  const [copyStatus, setCopyStatus] = useState('');
  useEffect(() => {
    setReport('');
    setCopyStatus('');
  }, [error, scope, phase]);
  async function copy() {
    try {
      await navigator.clipboard.writeText(report);
      setCopyStatus('已复制诊断摘要');
    } catch {
      setCopyStatus('剪贴板不可用，请选中下方摘要手动复制');
    }
  }
  return (
    <section className="termrill-error" aria-label={`${scope}错误`}>
      <div className="termrill-error-heading" role="alert">
        <AlertCircle size={16} />
        <strong>{diagnosis.title}</strong>
      </div>
      <p className="termrill-error-message">{error}</p>
      <p className="termrill-error-advice">{diagnosis.advice}</p>
      <div className="termrill-error-actions">
        {recovery && (
          <button type="button" disabled={disabled} onClick={recovery.run}>
            {recovery.label}
          </button>
        )}
        <button
          type="button"
          onClick={() =>
            setReport(
              report
                ? ''
                : diagnosticReport(
                    scope,
                    diagnosis.phase ?? phase,
                    diagnosis.code,
                    diagnosis.title,
                  ),
            )
          }
        >
          {report ? '收起诊断摘要' : '查看诊断摘要'}
        </button>
      </div>
      {report && (
        <div className="termrill-diagnostic">
          <pre tabIndex={0}>{report}</pre>
          <button type="button" onClick={() => void copy()}>
            <Copy size={14} />
            复制诊断摘要
          </button>
          <span role="status">{copyStatus}</span>
        </div>
      )}
    </section>
  );
}
