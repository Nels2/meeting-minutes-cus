import {
  AlignmentType,
  BorderStyle,
  Document,
  HeadingLevel,
  Packer,
  Paragraph,
  ShadingType,
  Table,
  TableCell,
  TableLayoutType,
  TableRow,
  TextRun,
  WidthType,
} from 'docx';
import { jsPDF } from 'jspdf';
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import { Summary, Transcript } from '@/types';

export type MeetingExportFormat = 'markdown' | 'docx' | 'pdf';

export interface MeetingExportBundle {
  meetingId: string;
  title: string;
  createdAt?: string;
  customContext?: string;
  summaryMarkdown?: string;
  transcripts: Transcript[];
}

const EXPORT_FILTERS: Record<MeetingExportFormat, { name: string; extensions: string[] }> = {
  markdown: { name: 'Markdown', extensions: ['md'] },
  docx: { name: 'Word Document', extensions: ['docx'] },
  pdf: { name: 'PDF', extensions: ['pdf'] },
};

function safeFileName(name: string): string {
  return name
    .replace(/[<>:"/\\|?*\u0000-\u001F]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, 120) || 'meeting-export';
}

function formatDate(value?: string): string {
  if (!value) return 'Unknown';
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return value;
  return parsed.toLocaleString();
}

function formatTranscriptTime(seconds: number | undefined, fallbackTimestamp: string): string {
  if (seconds === undefined || seconds === null) return fallbackTimestamp;
  const totalSeconds = Math.floor(seconds);
  const minutes = Math.floor(totalSeconds / 60);
  const remainingSeconds = totalSeconds % 60;
  return `[${minutes.toString().padStart(2, '0')}:${remainingSeconds.toString().padStart(2, '0')}]`;
}

function formatSpeaker(speaker?: string): string {
  return speaker ? `${speaker}: ` : '';
}

export function summaryToMarkdown(aiSummary: Summary | null | undefined): string {
  if (!aiSummary) return '';
  const summary = aiSummary as any;

  if (typeof summary.markdown === 'string' && summary.markdown.trim()) {
    return summary.markdown.trim();
  }

  return Object.entries(summary)
    .filter(([key]) => !['markdown', 'summary_json', '_section_order', 'MeetingName'].includes(key))
    .map(([, section]) => {
      if (!section || typeof section !== 'object') return '';
      const candidate = section as any;
      if (!candidate.title || !Array.isArray(candidate.blocks)) return '';
      const body = candidate.blocks
        .map((block: any) => `- ${block?.content ?? ''}`.trim())
        .filter(Boolean)
        .join('\n');
      return `## ${candidate.title}\n\n${body}`;
    })
    .filter(Boolean)
    .join('\n\n')
    .trim();
}

export function buildMeetingMarkdown(bundle: MeetingExportBundle): string {
  const lines: string[] = [
    `# ${bundle.title}`,
    '',
    `**Meeting ID:** ${bundle.meetingId}`,
    `**Created:** ${formatDate(bundle.createdAt)}`,
    `**Exported:** ${formatDate(new Date().toISOString())}`,
  ];

  if (bundle.customContext?.trim()) {
    lines.push('', '## Additional Context', '', bundle.customContext.trim());
  }

  if (bundle.summaryMarkdown?.trim()) {
    lines.push('', '## Summary', '', bundle.summaryMarkdown.trim());
  }

  lines.push('', '## Transcript', '');

  if (!bundle.transcripts.length) {
    lines.push('_No transcript segments are available._');
  } else {
    for (const transcript of bundle.transcripts) {
      const timestamp = formatTranscriptTime(transcript.audio_start_time, transcript.timestamp);
      lines.push(`${timestamp} ${formatSpeaker(transcript.speaker)}${transcript.text}`);
    }
  }

  return `${lines.join('\n')}\n`;
}

type ParsedMarkdownBlock =
  | { type: 'heading1' | 'heading2' | 'heading3'; text: string }
  | { type: 'heading'; level: number; text: string }
  | { type: 'bullet'; text: string; indent: number; checked?: boolean }
  | { type: 'ordered'; text: string; indent: number; number: string }
  | { type: 'quote'; text: string }
  | { type: 'code'; text: string }
  | { type: 'table'; headers: string[]; rows: string[][] }
  | { type: 'paragraph'; text: string };

interface TranscriptExportRow {
  timestamp: string;
  speaker: string;
  text: string;
}

function cleanInlineMarkdown(value: string): string {
  return value
    .replace(/\[([^\]]+)\]\(([^)]+)\)/g, '$1 ($2)')
    .replace(/`([^`]+)`/g, '$1')
    .replace(/(\*\*|__|\*|_)/g, '')
    .trim();
}

function isMarkdownTableRow(line: string): boolean {
  return /^\s*\|.*\|\s*$/.test(line);
}

function isMarkdownTableSeparator(line: string): boolean {
  if (!isMarkdownTableRow(line)) return false;
  const cells = splitMarkdownTableRow(line);
  return cells.length > 0 && cells.every(cell => /^:?-{3,}:?$/.test(cell.replace(/\s+/g, '')));
}

function splitMarkdownTableRow(line: string): string[] {
  return line
    .trim()
    .replace(/^\|/, '')
    .replace(/\|$/, '')
    .split('|')
    .map(cell => cleanInlineMarkdown(cell));
}

function normalizeTableRow(cells: string[], columnCount: number): string[] {
  if (cells.length === columnCount) return cells;
  if (cells.length > columnCount) return cells.slice(0, columnCount);
  return [...cells, ...Array.from({ length: columnCount - cells.length }, () => '')];
}

function parseMarkdownBlocks(markdown?: string): ParsedMarkdownBlock[] {
  if (!markdown?.trim()) return [];
  const blocks: ParsedMarkdownBlock[] = [];
  const paragraphBuffer: string[] = [];
  const lines = markdown.split('\n');

  const flushParagraph = () => {
    const text = cleanInlineMarkdown(paragraphBuffer.join(' '));
    if (text) blocks.push({ type: 'paragraph', text });
    paragraphBuffer.length = 0;
  };

  const pushListContinuation = (line: string): boolean => {
    const previous = blocks[blocks.length - 1];
    if ((previous?.type === 'bullet' || previous?.type === 'ordered') && /^\s{2,}\S/.test(line)) {
      previous.text = cleanInlineMarkdown(`${previous.text} ${line.trim()}`);
      return true;
    }
    return false;
  };

  for (let index = 0; index < lines.length; index += 1) {
    const rawLine = lines[index];
    const expandedLine = rawLine.replace(/\t/g, '    ');
    const line = rawLine.trim();

    if (!line || line === '---') {
      flushParagraph();
      continue;
    }

    if (/^(```|~~~)/.test(line)) {
      flushParagraph();
      const fence = line.slice(0, 3);
      const codeLines: string[] = [];
      index += 1;

      while (index < lines.length && !lines[index].trim().startsWith(fence)) {
        codeLines.push(lines[index]);
        index += 1;
      }

      const text = codeLines.join('\n').trim();
      if (text) blocks.push({ type: 'code', text });
      continue;
    }

    const heading = /^(#{1,6})\s+(.+)$/.exec(line);
    if (heading) {
      flushParagraph();
      const depth = heading[1].length;
      if (depth <= 3) {
        blocks.push({
          type: depth === 1 ? 'heading1' : depth === 2 ? 'heading2' : 'heading3',
          text: cleanInlineMarkdown(heading[2]),
        });
      } else {
        blocks.push({ type: 'heading', level: depth, text: cleanInlineMarkdown(heading[2]) });
      }
      continue;
    }

    const bullet = /^(\s*)[-*+]\s+(?:\[([ xX])\]\s+)?(.+)$/.exec(expandedLine);
    if (bullet) {
      flushParagraph();
      blocks.push({
        type: 'bullet',
        indent: Math.floor(bullet[1].length / 2),
        checked: bullet[2] ? bullet[2].toLowerCase() === 'x' : undefined,
        text: cleanInlineMarkdown(bullet[3]),
      });
      continue;
    }

    const ordered = /^(\s*)(\d+)[.)]\s+(.+)$/.exec(expandedLine);
    if (ordered) {
      flushParagraph();
      blocks.push({
        type: 'ordered',
        indent: Math.floor(ordered[1].length / 2),
        number: ordered[2],
        text: cleanInlineMarkdown(ordered[3]),
      });
      continue;
    }

    const quote = /^>\s?(.+)$/.exec(line);
    if (quote) {
      flushParagraph();
      blocks.push({ type: 'quote', text: cleanInlineMarkdown(quote[1]) });
      continue;
    }

    const nextLine = lines[index + 1]?.trim() ?? '';
    if (isMarkdownTableRow(line) && isMarkdownTableSeparator(nextLine)) {
      flushParagraph();
      const headers = splitMarkdownTableRow(line);
      const columnCount = headers.length;
      const rows: string[][] = [];
      index += 2;

      while (index < lines.length) {
        const rowLine = lines[index].trim();
        if (!isMarkdownTableRow(rowLine) || isMarkdownTableSeparator(rowLine)) {
          index -= 1;
          break;
        }

        rows.push(normalizeTableRow(splitMarkdownTableRow(rowLine), columnCount));
        index += 1;
      }

      if (columnCount > 0) {
        blocks.push({
          type: 'table',
          headers,
          rows,
        });
      }
      continue;
    }

    if (pushListContinuation(expandedLine)) {
      continue;
    }

    paragraphBuffer.push(line);
  }

  flushParagraph();
  return blocks;
}

function transcriptRows(bundle: MeetingExportBundle): TranscriptExportRow[] {
  return bundle.transcripts.map(transcript => ({
    timestamp: formatTranscriptTime(transcript.audio_start_time, transcript.timestamp),
    speaker: transcript.speaker?.trim() || 'Speaker',
    text: transcript.text.trim(),
  })).filter(row => row.text.length > 0);
}

function inlineTextRuns(value: string, options: { size?: number; color?: string } = {}): TextRun[] {
  const runs: TextRun[] = [];
  const pattern = /(\*\*[^*]+\*\*|__[^_]+__)/g;
  let cursor = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(value)) !== null) {
    if (match.index > cursor) {
      runs.push(new TextRun({
        text: cleanInlineMarkdown(value.slice(cursor, match.index)),
        size: options.size,
        color: options.color,
      }));
    }

    runs.push(new TextRun({
      text: cleanInlineMarkdown(match[0]),
      bold: true,
      size: options.size,
      color: options.color,
    }));
    cursor = match.index + match[0].length;
  }

  if (cursor < value.length) {
    runs.push(new TextRun({
      text: cleanInlineMarkdown(value.slice(cursor)),
      size: options.size,
      color: options.color,
    }));
  }

  return runs.length ? runs : [new TextRun({ text: cleanInlineMarkdown(value), size: options.size, color: options.color })];
}

function sectionHeading(text: string): Paragraph {
  return new Paragraph({
    text,
    heading: HeadingLevel.HEADING_1,
    spacing: { before: 360, after: 160 },
    thematicBreak: true,
  });
}

const docxTableBorders = {
  top: { style: BorderStyle.SINGLE, size: 1, color: 'CBD5E1' },
  bottom: { style: BorderStyle.SINGLE, size: 1, color: 'CBD5E1' },
  left: { style: BorderStyle.SINGLE, size: 1, color: 'CBD5E1' },
  right: { style: BorderStyle.SINGLE, size: 1, color: 'CBD5E1' },
  insideHorizontal: { style: BorderStyle.SINGLE, size: 1, color: 'E2E8F0' },
  insideVertical: { style: BorderStyle.SINGLE, size: 1, color: 'E2E8F0' },
};

function tableCellParagraph(text: string, bold = false): Paragraph {
  return new Paragraph({
    children: [new TextRun({
      text: cleanInlineMarkdown(text) || ' ',
      bold,
      size: 18,
      color: bold ? '0F172A' : '1F2937',
    })],
    spacing: { after: 0, line: 230 },
  });
}

function markdownTableToDocxTable(block: Extract<ParsedMarkdownBlock, { type: 'table' }>): Table {
  const columnCount = Math.max(1, block.headers.length);
  const columnWidth = Math.floor(100 / columnCount);
  const cellWidth = { size: columnWidth, type: WidthType.PERCENTAGE };

  const makeCell = (text: string, header = false) => new TableCell({
    width: cellWidth,
    margins: { top: 90, bottom: 90, left: 100, right: 100 },
    shading: header
      ? { type: ShadingType.CLEAR, fill: 'F1F5F9', color: 'auto' }
      : undefined,
    children: [tableCellParagraph(text, header)],
  });

  return new Table({
    width: { size: 100, type: WidthType.PERCENTAGE },
    layout: TableLayoutType.FIXED,
    borders: docxTableBorders,
    rows: [
      new TableRow({
        tableHeader: true,
        children: block.headers.map(header => makeCell(header, true)),
      }),
      ...block.rows.map(row => new TableRow({
        children: normalizeTableRow(row, columnCount).map(cell => makeCell(cell)),
      })),
    ],
  });
}

function markdownBlockToDocxElements(block: ParsedMarkdownBlock): (Paragraph | Table)[] {
  if (block.type === 'heading1' || block.type === 'heading2') {
    return [new Paragraph({
      text: block.text,
      heading: HeadingLevel.HEADING_2,
      spacing: { before: 220, after: 90 },
    })];
  }

  if (block.type === 'heading3') {
    return [new Paragraph({
      children: [new TextRun({ text: block.text, bold: true, size: 23, color: '334155' })],
      spacing: { before: 160, after: 70 },
    })];
  }

  if (block.type === 'heading') {
    return [new Paragraph({
      children: [new TextRun({ text: block.text, bold: true, size: 21, color: '334155' })],
      spacing: { before: 140, after: 60 },
    })];
  }

  if (block.type === 'bullet') {
    return [new Paragraph({
      children: inlineTextRuns(
        block.checked === undefined ? block.text : `${block.checked ? '[x]' : '[ ]'} ${block.text}`,
        { size: 21, color: '1F2937' }
      ),
      bullet: { level: 0 },
      indent: { left: 420 + block.indent * 240, hanging: 180 },
      spacing: { after: 80 },
    })];
  }

  if (block.type === 'ordered') {
    return [new Paragraph({
      children: inlineTextRuns(`${block.number}. ${block.text}`, { size: 21, color: '1F2937' }),
      indent: { left: 240 + block.indent * 240 },
      spacing: { after: 80 },
    })];
  }

  if (block.type === 'table') {
    return [
      markdownTableToDocxTable(block),
      new Paragraph({ text: '', spacing: { after: 140 } }),
    ];
  }

  if (block.type === 'quote') {
    return [new Paragraph({
      children: inlineTextRuns(block.text, { size: 20, color: '475569' }),
      indent: { left: 300 },
      spacing: { after: 120 },
    })];
  }

  if (block.type === 'code') {
    return [new Paragraph({
      children: [new TextRun({ text: block.text, font: 'Courier New', size: 18, color: '334155' })],
      spacing: { after: 130 },
    })];
  }

  return [new Paragraph({
    children: inlineTextRuns(block.text, { size: 21, color: '1F2937' }),
    spacing: { after: 130 },
    alignment: AlignmentType.LEFT,
  })];
}

async function bundleToDocx(bundle: MeetingExportBundle): Promise<Uint8Array> {
  const children: (Paragraph | Table)[] = [
    new Paragraph({
      text: bundle.title,
      heading: HeadingLevel.TITLE,
      spacing: { after: 180 },
    }),
    new Paragraph({
      children: [
        new TextRun({ text: 'Meeting ID: ', bold: true, color: '475569', size: 19 }),
        new TextRun({ text: bundle.meetingId, color: '475569', size: 19 }),
      ],
      spacing: { after: 40 },
    }),
    new Paragraph({
      children: [
        new TextRun({ text: 'Created: ', bold: true, color: '475569', size: 19 }),
        new TextRun({ text: formatDate(bundle.createdAt), color: '475569', size: 19 }),
      ],
      spacing: { after: 40 },
    }),
    new Paragraph({
      children: [
        new TextRun({ text: 'Exported: ', bold: true, color: '475569', size: 19 }),
        new TextRun({ text: formatDate(new Date().toISOString()), color: '475569', size: 19 }),
      ],
      spacing: { after: 220 },
    }),
  ];

  if (bundle.customContext?.trim()) {
    children.push(
      sectionHeading('Additional Context'),
      new Paragraph({
        children: inlineTextRuns(bundle.customContext.trim(), { size: 21, color: '1F2937' }),
        spacing: { after: 140 },
      })
    );
  }

  const summaryBlocks = parseMarkdownBlocks(bundle.summaryMarkdown);
  if (summaryBlocks.length) {
    children.push(sectionHeading('Summary'));
    summaryBlocks.forEach(block => children.push(...markdownBlockToDocxElements(block)));
  }

  children.push(sectionHeading('Transcript'));
  const rows = transcriptRows(bundle);
  if (!rows.length) {
    children.push(new Paragraph({
      children: [new TextRun({ text: 'No transcript segments are available.', italics: true, color: '64748B' })],
    }));
  } else {
    rows.forEach(row => {
      children.push(new Paragraph({
        children: [
          new TextRun({ text: row.timestamp, bold: true, color: '2563EB', size: 18 }),
          new TextRun({ text: `  ${row.speaker}: `, bold: true, color: '334155', size: 18 }),
          new TextRun({ text: row.text, size: 20, color: '1F2937' }),
        ],
        spacing: { before: 90, after: 80 },
      }));
    });
  }

  const document = new Document({
    styles: {
      default: {
        document: {
          run: { font: 'Aptos', size: 21, color: '1F2937' },
          paragraph: { spacing: { line: 276 } },
        },
      },
    },
    sections: [
      {
        properties: {
          page: {
            margin: {
              top: 900,
              right: 900,
              bottom: 900,
              left: 900,
            },
          },
        },
        children,
      },
    ],
  });

  const blob = await Packer.toBlob(document);
  return new Uint8Array(await blob.arrayBuffer());
}

function bundleToPdf(bundle: MeetingExportBundle): Uint8Array {
  const pdf = new jsPDF({ unit: 'pt', format: 'letter' });
  const pageWidth = pdf.internal.pageSize.getWidth();
  const pageHeight = pdf.internal.pageSize.getHeight();
  const marginX = 54;
  const marginTop = 54;
  const marginBottom = 60;
  const maxWidth = pageWidth - marginX * 2;
  let y = marginTop;
  let pageNumber = 1;

  const drawFooter = () => {
    pdf.setFont('helvetica', 'normal');
    pdf.setFontSize(8);
    pdf.setTextColor(120, 130, 145);
    pdf.setDrawColor(226, 232, 240);
    pdf.line(marginX, pageHeight - 36, pageWidth - marginX, pageHeight - 36);
    pdf.text('Meetily export', marginX, pageHeight - 22);
    pdf.text(`Page ${pageNumber}`, pageWidth - marginX, pageHeight - 22, { align: 'right' });
  };

  const addPage = () => {
    drawFooter();
    pdf.addPage();
    pageNumber += 1;
    y = marginTop;
  };

  const remainingPageHeight = () => pageHeight - marginBottom - y;

  const ensureSpace = (height: number) => {
    if (y + height > pageHeight - marginBottom) {
      addPage();
    }
  };

  const splitLines = (
    text: string,
    width: number,
    options: { fontSize?: number; fontStyle?: 'normal' | 'bold' | 'italic' } = {}
  ): string[] => {
    if (options.fontStyle || options.fontSize) {
      pdf.setFont('helvetica', options.fontStyle ?? 'normal');
      pdf.setFontSize(options.fontSize ?? 10);
    }

    const breakLongToken = (token: string): string[] => {
      if (pdf.getTextWidth(token) <= width) return [token];

      const chunks: string[] = [];
      let remaining = token;

      while (remaining.length > 0) {
        let low = 1;
        let high = remaining.length;
        let best = 1;

        while (low <= high) {
          const mid = Math.floor((low + high) / 2);
          if (pdf.getTextWidth(remaining.slice(0, mid)) <= width) {
            best = mid;
            low = mid + 1;
          } else {
            high = mid - 1;
          }
        }

        chunks.push(remaining.slice(0, best));
        remaining = remaining.slice(best);
      }

      return chunks;
    };

    const lines: string[] = [];
    const paragraphs = text
      .replace(/\t/g, ' ')
      .split(/\r?\n/)
      .map(paragraph => paragraph.replace(/\s+/g, ' ').trim());

    paragraphs.forEach(paragraph => {
      if (!paragraph) {
        if (lines.length) lines.push('');
        return;
      }

      let current = '';
      const tokens = paragraph.split(' ').flatMap(breakLongToken);

      tokens.forEach(token => {
        const candidate = current ? `${current} ${token}` : token;
        if (!current || pdf.getTextWidth(candidate) <= width) {
          current = candidate;
        } else {
          lines.push(current);
          current = token;
        }
      });

      if (current) lines.push(current);
    });

    return lines.length ? lines : [''];
  };

  const drawRule = (offsetY = 0) => {
    pdf.setDrawColor(226, 232, 240);
    pdf.setLineWidth(0.6);
    pdf.line(marginX, y + offsetY, pageWidth - marginX, y + offsetY);
  };

  const drawWrappedLines = (
    lines: string[],
    options: {
      x?: number;
      fontSize?: number;
      lineHeight?: number;
      fontStyle?: 'normal' | 'bold' | 'italic';
      color?: [number, number, number];
      after?: number;
      firstLinePrefix?: string;
      prefixWidth?: number;
    } = {}
  ) => {
    const fontSize = options.fontSize ?? 10;
    const lineHeight = options.lineHeight ?? 14.5;
    const x = options.x ?? marginX;
    const after = options.after ?? 0;
    let index = 0;

    pdf.setFont('helvetica', options.fontStyle ?? 'normal');
    pdf.setFontSize(fontSize);
    pdf.setTextColor(...(options.color ?? [31, 41, 55]));

    while (index < lines.length) {
      if (remainingPageHeight() < lineHeight) {
        addPage();
      }

      const availableLines = Math.max(1, Math.floor(remainingPageHeight() / lineHeight));
      const chunk = lines.slice(index, index + availableLines);

      if (options.firstLinePrefix && index === 0) {
        pdf.text(options.firstLinePrefix, x, y);
        pdf.text(chunk[0], x + (options.prefixWidth ?? 0), y);
        if (chunk.length > 1) {
          pdf.text(chunk.slice(1), x, y + lineHeight);
        }
      } else {
        pdf.text(chunk, x, y);
      }

      y += chunk.length * lineHeight;
      index += chunk.length;
    }

    y += after;
  };

  const drawTextBlock = (
    text: string,
    options: {
      x?: number;
      width?: number;
      fontSize?: number;
      lineHeight?: number;
      fontStyle?: 'normal' | 'bold' | 'italic';
      color?: [number, number, number];
      after?: number;
    } = {}
  ) => {
    const fontSize = options.fontSize ?? 10.5;
    const lineHeight = options.lineHeight ?? 15;
    const x = options.x ?? marginX;
    const width = options.width ?? maxWidth;
    const wrapped = splitLines(text, width, { fontSize, fontStyle: options.fontStyle });
    drawWrappedLines(wrapped, {
      x,
      fontSize,
      lineHeight,
      fontStyle: options.fontStyle,
      color: options.color,
      after: options.after ?? 4,
    });
  };

  const drawSectionHeading = (heading: string) => {
    ensureSpace(42);
    if (y > marginTop + 6) y += 10;
    pdf.setFont('helvetica', 'bold');
    pdf.setFontSize(13);
    pdf.setTextColor(15, 23, 42);
    pdf.text(heading, marginX, y);
    y += 7;
    pdf.setDrawColor(203, 213, 225);
    pdf.setLineWidth(0.7);
    pdf.line(marginX, y, pageWidth - marginX, y);
    y += 15;
  };

  const drawBullet = (block: Extract<ParsedMarkdownBlock, { type: 'bullet' }>) => {
    const indent = Math.min(block.indent, 4) * 14;
    const bulletX = marginX + 4 + indent;
    const textX = marginX + 18 + indent;
    const width = maxWidth - 18 - indent;
    const marker = block.checked === undefined ? '-' : block.checked ? '[x]' : '[ ]';
    const text = block.text;
    const wrapped = splitLines(text, width, { fontSize: 10 });
    ensureSpace(Math.min(30, wrapped.length * 14 + 6));

    pdf.setFont('helvetica', 'bold');
    pdf.setFontSize(9.8);
    pdf.setTextColor(37, 99, 235);
    pdf.text(marker, bulletX, y);

    drawWrappedLines(wrapped, {
      x: textX,
      fontSize: 10,
      lineHeight: 14.2,
      color: [31, 41, 55],
      after: 5,
    });
  };

  const drawOrderedItem = (block: Extract<ParsedMarkdownBlock, { type: 'ordered' }>) => {
    const indent = Math.min(block.indent, 4) * 14;
    const marker = `${block.number}.`;
    const markerWidth = Math.max(18, pdf.getTextWidth(marker) + 6);
    const markerX = marginX + 4 + indent;
    const textX = markerX + markerWidth;
    const width = maxWidth - (textX - marginX);
    const wrapped = splitLines(block.text, width, { fontSize: 10 });
    ensureSpace(Math.min(30, wrapped.length * 14 + 6));

    pdf.setFont('helvetica', 'bold');
    pdf.setFontSize(9.8);
    pdf.setTextColor(37, 99, 235);
    pdf.text(marker, markerX, y);

    drawWrappedLines(wrapped, {
      x: textX,
      fontSize: 10,
      lineHeight: 14.2,
      color: [31, 41, 55],
      after: 5,
    });
  };

  const drawMarkdownTable = (block: Extract<ParsedMarkdownBlock, { type: 'table' }>) => {
    const columnCount = Math.max(1, block.headers.length);
    const headers = normalizeTableRow(block.headers, columnCount);
    const normalizedHeaders = headers.map(header => header.toLowerCase().replace(/[^a-z0-9]/g, ''));
    const primaryColumnIndex = normalizedHeaders.findIndex(header =>
      /^(action|actionitem|task|todo|item|description|nextstep|nextsteps)$/.test(header)
    );
    const hasActionMetadata = normalizedHeaders.some(header =>
      /^(owner|assignee|responsible|due|duedate|deadline|date|status|priority)$/.test(header)
    );

    if (primaryColumnIndex >= 0 && hasActionMetadata) {
      block.rows.forEach((row, rowIndex) => {
        const cells = normalizeTableRow(row, columnCount);
        const actionText = cells[primaryColumnIndex] || `Action item ${rowIndex + 1}`;
        const metadata = cells
          .map((value, index) => ({ label: headers[index], value }))
          .filter((entry, index) => index !== primaryColumnIndex && entry.value.trim());

        ensureSpace(40);
        drawTextBlock(`${rowIndex + 1}. ${actionText}`, {
          fontSize: 10.2,
          lineHeight: 14,
          fontStyle: 'bold',
          color: [15, 23, 42],
          after: 4,
        });

        metadata.forEach(entry => {
          ensureSpace(24);
          pdf.setFont('helvetica', 'bold');
          pdf.setFontSize(7.2);
          pdf.setTextColor(71, 85, 105);
          pdf.text(`${entry.label.toUpperCase()}:`, marginX + 14, y);

          drawTextBlock(entry.value, {
            x: marginX + 92,
            width: maxWidth - 92,
            fontSize: 8.8,
            lineHeight: 11.8,
            color: [51, 65, 85],
            after: 4,
          });
        });

        drawRule();
        y += 12;
      });
      return;
    }

    block.rows.forEach((row, rowIndex) => {
      const cells = normalizeTableRow(row, columnCount);
      ensureSpace(44);

      pdf.setFont('helvetica', 'bold');
      pdf.setFontSize(8.6);
      pdf.setTextColor(37, 99, 235);
      pdf.text(`Item ${rowIndex + 1}`, marginX, y);
      y += 13;

      cells.forEach((cell, cellIndex) => {
        const header = headers[cellIndex] || `Column ${cellIndex + 1}`;
        const value = cell || '-';
        ensureSpace(28);

        pdf.setFont('helvetica', 'bold');
        pdf.setFontSize(7.4);
        pdf.setTextColor(71, 85, 105);
        pdf.text(header.toUpperCase(), marginX, y);
        y += 10;

        drawTextBlock(value, {
          fontSize: 9.4,
          lineHeight: 12.8,
          after: 7,
        });
      });

      drawRule();
      y += 12;
    });
  };

  const drawSummaryBlock = (block: ParsedMarkdownBlock) => {
    if (block.type === 'heading1' || block.type === 'heading2') {
      ensureSpace(30);
      drawTextBlock(block.text, {
        fontSize: 11.5,
        lineHeight: 15.5,
        fontStyle: 'bold',
        color: [30, 64, 175],
        after: 7,
      });
      return;
    }

    if (block.type === 'heading3' || block.type === 'heading') {
      ensureSpace(26);
      drawTextBlock(block.text, {
        fontSize: 10.5,
        lineHeight: 14,
        fontStyle: 'bold',
        color: [51, 65, 85],
        after: 6,
      });
      return;
    }

    if (block.type === 'bullet') {
      drawBullet(block);
      return;
    }

    if (block.type === 'ordered') {
      drawOrderedItem(block);
      return;
    }

    if (block.type === 'table') {
      drawMarkdownTable(block);
      return;
    }

    if (block.type === 'quote') {
      drawTextBlock(block.text, {
        x: marginX + 16,
        width: maxWidth - 16,
        fontSize: 9.8,
        lineHeight: 13.8,
        fontStyle: 'italic',
        color: [71, 85, 105],
        after: 8,
      });
      return;
    }

    if (block.type === 'code') {
      drawTextBlock(block.text, {
        x: marginX + 12,
        width: maxWidth - 24,
        fontSize: 8.8,
        lineHeight: 12.5,
        color: [51, 65, 85],
        after: 8,
      });
      return;
    }

    drawTextBlock(block.text, { fontSize: 10.2, lineHeight: 14.5, after: 8 });
  };

  const drawTranscriptRow = (row: TranscriptExportRow) => {
    const labelWidth = 112;
    const gap = 14;
    const textX = marginX + labelWidth + gap;
    const textWidth = maxWidth - labelWidth - gap;
    const labelLines = splitLines(`${row.timestamp}\n${row.speaker}`, labelWidth, { fontSize: 8.5, fontStyle: 'bold' });
    const textLines = splitLines(row.text, textWidth, { fontSize: 9.5 });
    const lineHeight = 13.5;
    const labelHeight = labelLines.length * 11.5;
    const firstBlockHeight = Math.max(labelHeight, Math.min(textLines.length, 3) * lineHeight);

    ensureSpace(Math.max(28, firstBlockHeight + 8));

    const entryTop = y;
    pdf.setFont('helvetica', 'bold');
    pdf.setFontSize(8.5);
    pdf.setTextColor(37, 99, 235);
    pdf.text(labelLines[0] ?? row.timestamp, marginX, y);

    if (labelLines.length > 1) {
      pdf.setFont('helvetica', 'normal');
      pdf.setTextColor(71, 85, 105);
      pdf.text(labelLines.slice(1), marginX, y + 11.5);
    }

    pdf.setFont('helvetica', 'normal');
    pdf.setFontSize(9.5);
    pdf.setTextColor(31, 41, 55);

    let index = 0;
    while (index < textLines.length) {
      if (remainingPageHeight() < lineHeight) {
        addPage();
      }

      const availableLines = Math.max(1, Math.floor(remainingPageHeight() / lineHeight));
      const chunk = textLines.slice(index, index + availableLines);
      pdf.text(chunk, textX, y);
      y += chunk.length * lineHeight;
      index += chunk.length;
    }

    y = Math.max(y, entryTop + labelHeight);
    y += 9;
    drawRule();
    y += 9;
  };

  pdf.setFont('helvetica', 'bold');
  pdf.setFontSize(18);
  pdf.setTextColor(15, 23, 42);
  const titleLines = splitLines(bundle.title, maxWidth, { fontSize: 18, fontStyle: 'bold' });
  pdf.text(titleLines, marginX, y);
  y += titleLines.length * 22 + 10;
  pdf.setDrawColor(37, 99, 235);
  pdf.setLineWidth(1);
  pdf.line(marginX, y, pageWidth - marginX, y);
  y += 18;

  [
    ['Meeting ID', bundle.meetingId],
    ['Created', formatDate(bundle.createdAt)],
    ['Exported', formatDate(new Date().toISOString())],
  ].forEach(([label, value]) => {
    pdf.setFont('helvetica', 'bold');
    pdf.setFontSize(8.7);
    pdf.setTextColor(71, 85, 105);
    pdf.text(`${label}:`, marginX, y);
    pdf.setFont('helvetica', 'normal');
    const valueLines = splitLines(value, maxWidth - 68, { fontSize: 8.7 });
    pdf.text(valueLines, marginX + 68, y);
    y += Math.max(13, valueLines.length * 11.5);
  });
  y += 10;

  if (bundle.customContext?.trim()) {
    drawSectionHeading('Additional Context');
    drawTextBlock(bundle.customContext.trim(), {
      fontSize: 10.2,
      lineHeight: 14.5,
      after: 10,
    });
  }

  const summaryBlocks = parseMarkdownBlocks(bundle.summaryMarkdown);
  if (summaryBlocks.length) {
    drawSectionHeading('Summary');
    summaryBlocks.forEach(drawSummaryBlock);
  }

  drawSectionHeading('Transcript');
  const rows = transcriptRows(bundle);
  if (!rows.length) {
    drawTextBlock('No transcript segments are available.', {
      fontSize: 10.2,
      lineHeight: 14,
      color: [100, 116, 139],
      fontStyle: 'italic',
      after: 8,
    });
  } else {
    rows.forEach(drawTranscriptRow);
  }

  drawFooter();
  return new Uint8Array(pdf.output('arraybuffer'));
}

async function writeExportFile(filePath: string, contents: Uint8Array): Promise<void> {
  await invoke('write_export_file', {
    filePath,
    contents: Array.from(contents),
  });
}

export async function exportMeetingBundle(format: MeetingExportFormat, bundle: MeetingExportBundle): Promise<string | null> {
  const extension = EXPORT_FILTERS[format].extensions[0];
  const filePath = await save({
    defaultPath: `${safeFileName(bundle.title)}.${extension}`,
    filters: [EXPORT_FILTERS[format]],
  });

  if (!filePath) return null;

  const markdown = buildMeetingMarkdown(bundle);

  if (format === 'markdown') {
    await writeExportFile(filePath, new TextEncoder().encode(markdown));
  } else if (format === 'docx') {
    await writeExportFile(filePath, await bundleToDocx(bundle));
  } else {
    await writeExportFile(filePath, bundleToPdf(bundle));
  }

  return filePath;
}
