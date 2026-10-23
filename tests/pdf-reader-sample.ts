/** 无外部资源的有效 PDF 样本，供真实 PDF.js 解码与重新打开回归；不含用户文件。 */
function createSample() {
  const count = 50;
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>',
    `<< /Type /Pages /Count ${count} /Kids [${Array.from({ length: count }, (_, index) => `${5 + index * 2} 0 R`).join(' ')}] >>`,
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
    '<< /Type /Outlines /First 105 0 R /Last 108 0 R /Count 4 >>',
  ];
  for (let index = 0; index < count; index++) {
    const pageId = 5 + index * 2;
    const stream = `BT /F1 24 Tf 40 730 Td (TermRill PDF regression - page ${index + 1}) Tj ET`;
    objects.push(
      `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents ${pageId + 1} 0 R >>`,
    );
    objects.push(`<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`);
  }
  objects.push(
    '<< /Title (Chapter One - a long title for horizontal scrolling and navigation regression) /Parent 4 0 R /Dest [5 0 R /Fit] /Next 107 0 R /First 106 0 R /Last 106 0 R /Count 1 >>',
  );
  objects.push('<< /Title (Nested section) /Parent 105 0 R /Dest [9 0 R /Fit] >>');
  objects.push('<< /Title () /Parent 4 0 R /Prev 105 0 R /Next 108 0 R /Dest [13 0 R /Fit] >>');
  objects.push('<< /Title (Chapter Two) /Parent 4 0 R /Prev 107 0 R /Dest [23 0 R /Fit] >>');
  let pdf = '%PDF-1.7\n';
  const offsets = [0];
  objects.forEach((object, index) => {
    offsets.push(pdf.length);
    pdf += `${index + 1} 0 obj\n${object}\nendobj\n`;
  });
  const xref = pdf.length;
  pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  pdf += offsets
    .slice(1)
    .map((offset) => `${String(offset).padStart(10, '0')} 00000 n \n`)
    .join('');
  pdf += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF`;
  return new TextEncoder().encode(pdf).buffer;
}
export const pdfReaderSample = createSample();
