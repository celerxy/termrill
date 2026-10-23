import ErrorNotice from './ErrorNotice';
/** SVG 在图片上下文中解码；移除脚本、嵌入内容和外部资源，关闭释放内存 URL。 */
import { useEffect, useState } from 'react';
import DOMPurify from 'dompurify';

export default function SvgPreview({ content }: { content: string }) {
  const [url, setUrl] = useState('');
  const [error, setError] = useState('');
  useEffect(() => {
    setError('');
    setUrl('');
    const clean = DOMPurify.sanitize(content, {
      USE_PROFILES: { svg: true, svgFilters: true },
      FORBID_TAGS: [
        'script',
        'foreignObject',
        'image',
        'style',
        'animate',
        'set',
        'animateTransform',
      ],
      FORBID_ATTR: ['style'],
    });
    const parsed = new DOMParser().parseFromString(clean, 'image/svg+xml');
    if (parsed.querySelector('parsererror') || parsed.documentElement.localName !== 'svg') {
      setError('SVG 格式无效，可切换到源码修正');
      return;
    }
    for (const element of [parsed.documentElement, ...parsed.querySelectorAll('*')]) {
      for (const attribute of Array.from(element.attributes)) {
        const value = attribute.value.trim();
        if (
          (/href$/i.test(attribute.name) && !value.startsWith('#')) ||
          (/url\s*\(/i.test(value) && !/^url\(\s*#[\w-]+\s*\)$/i.test(value))
        )
          element.removeAttributeNode(attribute);
      }
    }
    const objectUrl = URL.createObjectURL(
      new Blob([new XMLSerializer().serializeToString(parsed.documentElement)], {
        type: 'image/svg+xml',
      }),
    );
    setUrl(objectUrl);
    return () => URL.revokeObjectURL(objectUrl);
  }, [content]);
  if (error) return <ErrorNotice error={error} scope="文件预览" />;
  return (
    <div className="document-image fit-image">
      {url && (
        <img
          src={url}
          alt="SVG 预览"
          style={{ maxWidth: '100%', maxHeight: '100%' }}
          onError={() => setError('SVG 解码失败，可切换到源码查看')}
        />
      )}
    </div>
  );
}
