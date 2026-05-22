import { Document, HeadingLevel, Packer, Paragraph, TextRun } from 'docx';
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

function paragraphFromMarkdownLine(line: string): Paragraph {
  if (line.startsWith('# ')) {
    return new Paragraph({
      text: line.replace(/^# /, ''),
      heading: HeadingLevel.TITLE,
    });
  }

  if (line.startsWith('## ')) {
    return new Paragraph({
      text: line.replace(/^## /, ''),
      heading: HeadingLevel.HEADING_1,
      spacing: { before: 240, after: 120 },
    });
  }

  if (line.startsWith('### ')) {
    return new Paragraph({
      text: line.replace(/^### /, ''),
      heading: HeadingLevel.HEADING_2,
      spacing: { before: 180, after: 80 },
    });
  }

  if (line.startsWith('- ')) {
    return new Paragraph({
      text: line.replace(/^- /, ''),
      bullet: { level: 0 },
      spacing: { after: 80 },
    });
  }

  return new Paragraph({
    children: [new TextRun(line.replace(/\*\*/g, ''))],
    spacing: { after: 100 },
  });
}

async function markdownToDocx(markdown: string): Promise<Uint8Array> {
  const paragraphs = markdown
    .split('\n')
    .map(line => line.trimEnd())
    .filter(line => line !== '---')
    .map(paragraphFromMarkdownLine);

  const document = new Document({
    sections: [
      {
        properties: {},
        children: paragraphs,
      },
    ],
  });

  const blob = await Packer.toBlob(document);
  return new Uint8Array(await blob.arrayBuffer());
}

function markdownToPdf(markdown: string): Uint8Array {
  const pdf = new jsPDF({ unit: 'pt', format: 'letter' });
  const pageWidth = pdf.internal.pageSize.getWidth();
  const pageHeight = pdf.internal.pageSize.getHeight();
  const marginX = 54;
  const marginTop = 58;
  const marginBottom = 54;
  const maxWidth = pageWidth - marginX * 2;
  let y = marginTop;
  let pageNumber = 1;

  const cleanInlineMarkdown = (value: string) =>
    value
      .replace(/\[([^\]]+)\]\(([^)]+)\)/g, '$1 ($2)')
      .replace(/(\*\*|__|\*|_|`)/g, '')
      .trimEnd();

  const drawFooter = () => {
    pdf.setFont('helvetica', 'normal');
    pdf.setFontSize(8);
    pdf.setTextColor(120, 130, 145);
    pdf.setDrawColor(226, 232, 240);
    pdf.line(marginX, pageHeight - 36, pageWidth - marginX, pageHeight - 36);
    pdf.text('Meetily export', marginX, pageHeight - 22);
    pdf.text(`Page ${pageNumber}`, pageWidth - marginX, pageHeight - 22, { align: 'right' });
  };

  const addPageIfNeeded = (height: number) => {
    if (y + height > pageHeight - marginBottom) {
      drawFooter();
      pdf.addPage();
      pageNumber += 1;
      y = marginTop;
    }
  };

  const writeWrappedText = (
    text: string,
    options: {
      x?: number;
      width?: number;
      fontSize?: number;
      lineHeight?: number;
      fontStyle?: 'normal' | 'bold';
      color?: [number, number, number];
      after?: number;
    } = {}
  ) => {
    const fontSize = options.fontSize ?? 10.5;
    const lineHeight = options.lineHeight ?? 15;
    const x = options.x ?? marginX;
    const width = options.width ?? maxWidth;
    const wrapped = pdf.splitTextToSize(text, width) as string[];

    addPageIfNeeded(wrapped.length * lineHeight);
    pdf.setFont('helvetica', options.fontStyle ?? 'normal');
    pdf.setFontSize(fontSize);
    pdf.setTextColor(...(options.color ?? [31, 41, 55]));
    pdf.text(wrapped, x, y);
    y += wrapped.length * lineHeight + (options.after ?? 4);
  };

  const lines = markdown.split('\n');
  for (const rawLine of lines) {
    const line = cleanInlineMarkdown(rawLine);

    if (line === '---') continue;
    if (!line.trim()) {
      y += 7;
      continue;
    }

    if (line.startsWith('# ')) {
      const title = line.replace(/^#\s+/, '');
      const titleLines = pdf.splitTextToSize(title, maxWidth) as string[];
      addPageIfNeeded(48);
      pdf.setFont('helvetica', 'bold');
      pdf.setFontSize(22);
      pdf.setTextColor(15, 23, 42);
      pdf.text(titleLines, marginX, y);
      y += titleLines.length * 28;
      pdf.setDrawColor(37, 99, 235);
      pdf.setLineWidth(1.2);
      pdf.line(marginX, y, pageWidth - marginX, y);
      y += 18;
      continue;
    }

    if (line.startsWith('## ')) {
      const heading = line.replace(/^##\s+/, '');
      addPageIfNeeded(42);
      y += 6;
      pdf.setFont('helvetica', 'bold');
      pdf.setFontSize(14);
      pdf.setTextColor(29, 78, 216);
      pdf.text(heading, marginX, y);
      y += 10;
      pdf.setDrawColor(226, 232, 240);
      pdf.setLineWidth(0.6);
      pdf.line(marginX, y, pageWidth - marginX, y);
      y += 14;
      continue;
    }

    if (line.startsWith('### ')) {
      writeWrappedText(line.replace(/^###\s+/, ''), {
        fontSize: 12,
        lineHeight: 16,
        fontStyle: 'bold',
        color: [51, 65, 85],
        after: 6,
      });
      continue;
    }

    if (line.startsWith('- ')) {
      const bulletText = line.replace(/^-+\s*/, '');
      const bulletX = marginX + 8;
      const textX = marginX + 22;
      const width = maxWidth - 22;
      const wrapped = pdf.splitTextToSize(bulletText, width) as string[];
      addPageIfNeeded(wrapped.length * 14 + 4);
      pdf.setFont('helvetica', 'bold');
      pdf.setFontSize(10);
      pdf.setTextColor(37, 99, 235);
      pdf.text('-', bulletX, y);
      pdf.setFont('helvetica', 'normal');
      pdf.setFontSize(10.5);
      pdf.setTextColor(31, 41, 55);
      pdf.text(wrapped, textX, y);
      y += wrapped.length * 14 + 4;
      continue;
    }

    if (/^Meeting ID:|^Created:|^Exported:/.test(line)) {
      writeWrappedText(line, {
        fontSize: 9.5,
        lineHeight: 13,
        color: [100, 116, 139],
        after: 2,
      });
      continue;
    }

    if (/^\[[0-9]{2}:[0-9]{2}\]/.test(line)) {
      writeWrappedText(line, {
        fontSize: 9.5,
        lineHeight: 13,
        color: [51, 65, 85],
        after: 3,
      });
      continue;
    }

    writeWrappedText(line);
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
    await writeExportFile(filePath, await markdownToDocx(markdown));
  } else {
    await writeExportFile(filePath, markdownToPdf(markdown));
  }

  return filePath;
}
