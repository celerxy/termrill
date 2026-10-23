/** CSV 只读表格：分页限制 DOM 数量；值原样展示，不执行公式或 HTML。 */
import { useMemo, useState } from 'react';
import { csvDelimiter, parseCsv } from './csvData';
import './csvPreview.css';
export default function CsvPreview({
  content,
  path,
  complete,
}: {
  content: string;
  path: string;
  complete: boolean;
}) {
  const [separator, setSeparator] = useState('auto');
  const [header, setHeader] = useState(true);
  const [query, setQuery] = useState('');
  const [page, setPage] = useState(0);
  const [detail, setDetail] = useState<{ title: string; value: string } | null>(null);
  const data = useMemo(
    () =>
      parseCsv(
        content,
        separator === 'auto'
          ? path.toLowerCase().endsWith('.tsv')
            ? '\t'
            : csvDelimiter(content)
          : separator,
        complete,
      ),
    [content, path, separator, complete],
  );
  const records = useMemo(
    () => data.rows.slice(header ? 1 : 0).map((cells, index) => ({ cells, index: index + 1 })),
    [data, header],
  );
  const filtered = useMemo(() => {
    const search = query.toLocaleLowerCase();
    return search
      ? records.filter(({ cells }) =>
          cells.some((value) => value.toLocaleLowerCase().includes(search)),
        )
      : records;
  }, [records, query]);
  const current = Math.min(page, Math.max(0, Math.ceil(filtered.length / 100) - 1));
  const count = data.rows.reduce((max, cells) => Math.max(max, cells.length), 0);
  const columns = Array.from({ length: count }, (_, index) =>
    header ? (data.rows[0]?.[index] ?? '') : `列 ${index + 1}`,
  );
  const visible = filtered.slice(current * 100, (current + 1) * 100);
  const widths = columns.map((name, column) => {
    let length = name.length;
    for (const row of data.rows.slice(0, 50)) {
      const value = row[column] ?? '';
      if (value.length > 80 || /[\r\n]/.test(value)) return 420;
      length = Math.max(length, value.length);
    }
    return Math.min(280, Math.max(120, length * 8 + 28));
  });
  return (
    <div className="csv-preview">
      <div className="csv-toolbar">
        <label>
          分隔符{' '}
          <select
            aria-label="CSV 分隔符"
            value={separator}
            onChange={(event) => {
              setSeparator(event.target.value);
              setPage(0);
            }}
          >
            <option value="auto">自动识别</option>
            <option value=",">逗号</option>
            <option value=";">分号</option>
            <option value={'\t'}>制表符</option>
          </select>
        </label>
        <label>
          <input
            type="checkbox"
            checked={header}
            onChange={(event) => {
              setHeader(event.target.checked);
              setPage(0);
            }}
          />
          首行作为表头
        </label>
        <input
          type="search"
          aria-label="筛选 CSV 表格"
          placeholder="搜索已加载的表格内容"
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setPage(0);
          }}
        />
        <span>
          {records.length} 条记录 · {count} 列{complete ? '' : ' · 加载中'}
        </span>
      </div>
      {data.warning && (
        <p className="csv-warning" role="status">
          {data.warning}
        </p>
      )}
      <div
        className="csv-table-scroll"
        tabIndex={0}
        aria-label="CSV 表格"
        data-stop-auto-loading={Boolean(data.warning)}
      >
        {columns.length ? (
          <table style={{ width: 60 + widths.reduce((sum, width) => sum + width, 0) }}>
            <colgroup>
              <col style={{ width: 60 }} />
              {widths.map((width, index) => (
                <col key={index} style={{ width }} />
              ))}
            </colgroup>
            <thead>
              <tr>
                <th scope="col">序号</th>
                {columns.map((name, index) => (
                  <th scope="col" key={index}>
                    {name || `列 ${index + 1}`}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {visible.map(({ cells, index }) => (
                <tr key={index}>
                  <th scope="row">{index}</th>
                  {columns.map((_, column) => (
                    <td key={column}>
                      <button
                        type="button"
                        className="csv-cell-value"
                        title="点击查看完整内容"
                        aria-label={`记录 ${index}，${columns[column] || `列 ${column + 1}`}，查看完整内容`}
                        onClick={() =>
                          setDetail({
                            title: `记录 ${index} · ${columns[column] || `列 ${column + 1}`}`,
                            value: cells[column] ?? '',
                          })
                        }
                      >
                        {(cells[column] ?? '').slice(0, 300)}
                      </button>
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <p className="csv-empty">{complete ? '文件没有可显示的记录' : '正在读取完整记录…'}</p>
        )}
        {columns.length > 0 && filtered.length === 0 && <p className="csv-empty">没有匹配的记录</p>}
      </div>
      {detail && (
        <section className="csv-cell-detail" aria-label="单元格完整内容">
          <header>
            <strong>{detail.title}</strong>
            <button type="button" onClick={() => setDetail(null)}>
              关闭详情
            </button>
          </header>
          <pre tabIndex={0}>{detail.value || '（空单元格）'}</pre>
        </section>
      )}
      <div className="csv-pagination">
        <button disabled={current === 0} onClick={() => setPage(current - 1)}>
          上一页
        </button>
        <span>
          {current + 1} / {Math.max(1, Math.ceil(filtered.length / 100))} · 每页 100 条
        </span>
        <button
          disabled={(current + 1) * 100 >= filtered.length}
          onClick={() => setPage(current + 1)}
        >
          下一页
        </button>
      </div>
    </div>
  );
}
