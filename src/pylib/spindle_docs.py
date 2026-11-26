"""Write Word (.docx) and Excel (.xlsx) files with only the Python standard library.

Spindle provides this module inside its sandbox, so scripts can create real Office files
without third-party packages:

    from spindle_docs import write_docx, write_xlsx
    write_docx("report.docx", "# Title\\n\\nSome **bold** text.\\n\\n- a point")
    write_xlsx("data.xlsx", [["Route", "Riders", "Share"], ["1", 1200, 0.25]],
               sheet="Ridership", formats={"Riders": "thousands", "Share": "percent"})
    write_pdf("report.pdf", "# Title\n\nSome **bold** text.\n\n- a point")

write_docx takes simple Markdown: # to ###### headings, paragraphs, - or * bullets,
1. numbered items, **bold**, *italic*, `code`, and | pipe | tables |.
write_xlsx takes a list of rows (numbers stay numbers), or a dict of sheet name to rows. The
first row is a bold, shaded, frozen header with filters, and columns are sized to fit. `formats`
maps a column (header text, letter or 0-based index) to an Excel number format or one of
"percent", "percent1", "currency", "thousands", "decimal", "integer", "date".
write_pdf takes the same Markdown as write_docx and lays it out on Letter pages.
"""

import datetime
import re
import zipfile
import zlib
from xml.sax.saxutils import escape

__all__ = ["write_docx", "write_xlsx", "write_pdf"]

_W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
_R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
_PKG = "http://schemas.openxmlformats.org/package/2006/relationships"
_CT = "http://schemas.openxmlformats.org/package/2006/content-types"


def _text(value):
    return escape(str(value)).replace('"', "&quot;")


def _runs(text):
    """Inline Markdown to WordprocessingML runs."""
    out = []
    pattern = re.compile(r"(\*\*[^*]+\*\*|\*[^*]+\*|`[^`]+`)")
    for part in pattern.split(text):
        if not part:
            continue
        props = ""
        if part.startswith("**") and part.endswith("**") and len(part) > 4:
            part, props = part[2:-2], "<w:b/>"
        elif part.startswith("`") and part.endswith("`") and len(part) > 2:
            part, props = part[1:-1], '<w:rFonts w:ascii="Consolas" w:hAnsi="Consolas"/>'
        elif part.startswith("*") and part.endswith("*") and len(part) > 2:
            part, props = part[1:-1], "<w:i/>"
        rpr = f"<w:rPr>{props}</w:rPr>" if props else ""
        out.append(f'<w:r>{rpr}<w:t xml:space="preserve">{_text(part)}</w:t></w:r>')
    return "".join(out)


def _para(text, style=None, indent=None):
    ppr = ""
    if style or indent:
        inner = f'<w:pStyle w:val="{style}"/>' if style else ""
        if indent:
            inner += f'<w:ind w:left="{indent}" w:hanging="360"/>'
        ppr = f"<w:pPr>{inner}</w:pPr>"
    return f"<w:p>{ppr}{_runs(text)}</w:p>"


def _table(rows):
    columns = max(len(r) for r in rows)
    width = 9360 // columns
    grid = "".join(f'<w:gridCol w:w="{width}"/>' for _ in range(columns))
    cells = []
    for i, row in enumerate(rows):
        row = list(row) + [""] * (columns - len(row))
        tcs = "".join(
            f'<w:tc><w:tcPr><w:tcW w:w="{width}" w:type="dxa"/></w:tcPr><w:p>{_runs(f"**{c}**" if i == 0 and c else c)}</w:p></w:tc>'
            for c in row
        )
        cells.append(f"<w:tr>{tcs}</w:tr>")
    borders = "".join(
        f'<w:{side} w:val="single" w:sz="4" w:space="0" w:color="999999"/>'
        for side in ("top", "left", "bottom", "right", "insideH", "insideV")
    )
    return f'<w:tbl><w:tblPr><w:tblW w:w="0" w:type="auto"/><w:tblBorders>{borders}</w:tblBorders></w:tblPr><w:tblGrid>{grid}</w:tblGrid>{"".join(cells)}</w:tbl>'


def _body(markdown):
    blocks, para, table = [], [], []

    def flush():
        if para:
            blocks.append(_para(" ".join(para)))
            para.clear()
        if table:
            rows = [r for r in table if not re.fullmatch(r"\|?[\s:|-]+\|?", r.strip())]
            parsed = [[c.strip() for c in r.strip().strip("|").split("|")] for r in rows]
            if parsed:
                blocks.append(_table(parsed))
                blocks.append("<w:p/>")
            table.clear()

    number = 0
    for line in str(markdown).splitlines():
        stripped = line.strip()
        heading = re.match(r"(#{1,6})\s+(.*)", stripped)
        bullet = re.match(r"[-*+]\s+(.*)", stripped)
        numbered = re.match(r"\d+[.)]\s+(.*)", stripped)
        if stripped.startswith("|"):
            if para:
                flush()
            table.append(stripped)
            continue
        if table:
            flush()
        if not stripped:
            flush()
            number = 0
        elif heading:
            flush()
            level = len(heading.group(1))
            blocks.append(_para(heading.group(2), "Title" if level == 1 and not blocks else f"Heading{min(level, 3)}"))
        elif bullet:
            flush()
            blocks.append(_para("•\t" + bullet.group(1), indent=720))
        elif numbered:
            flush()
            number += 1
            blocks.append(_para(f"{number}.\t" + numbered.group(1), indent=720))
        else:
            para.append(stripped)
    flush()
    return "".join(blocks) or "<w:p/>"


_STYLES = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="{_W}">
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:cs="Calibri"/><w:sz w:val="22"/></w:rPr></w:rPrDefault>
<w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:spacing w:after="240"/></w:pPr><w:rPr><w:sz w:val="48"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="360" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="80"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="28"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="200" w:after="60"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:sz w:val="24"/></w:rPr></w:style>
</w:styles>"""


def _extension(path, ext):
    """The path with the right extension, so write_docx("notes.md", ...) makes notes.docx."""
    path = str(path)
    stem, dot, current = path.rpartition(".")
    if dot and current.lower() == ext:
        return path
    if dot and "/" not in current and "\\" not in current and len(current) <= 5:
        return f"{stem}.{ext}"
    return f"{path}.{ext}"


def write_docx(path, markdown, title=None):
    """Write a .docx file from simple Markdown. Returns the path (always ending in .docx)."""
    path = _extension(path, "docx")
    document = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        f'<w:document xmlns:w="{_W}" xmlns:r="{_R}"><w:body>{_body(markdown)}'
        '<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>'
        "</w:body></w:document>"
    )
    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    core = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
        'xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" '
        'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">'
        f"<dc:title>{_text(title or '')}</dc:title><dc:creator>Spindle</dc:creator>"
        f'<dcterms:created xsi:type="dcterms:W3CDTF">{now}</dcterms:created></cp:coreProperties>'
    )
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr(
            "[Content_Types].xml",
            f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="{_CT}">'
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
            '<Default Extension="xml" ContentType="application/xml"/>'
            '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
            '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>'
            '<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>'
            "</Types>",
        )
        z.writestr(
            "_rels/.rels",
            f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{_PKG}">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>'
            '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>'
            "</Relationships>",
        )
        z.writestr(
            "word/_rels/document.xml.rels",
            f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{_PKG}">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>'
            "</Relationships>",
        )
        z.writestr("word/document.xml", document)
        z.writestr("word/styles.xml", _STYLES)
        z.writestr("docProps/core.xml", core)
    return path


def _column(n):
    name = ""
    n += 1
    while n:
        n, r = divmod(n - 1, 26)
        name = chr(65 + r) + name
    return name


_SHORTCUTS = {
    "percent": "0%",
    "percent1": "0.0%",
    "currency": "$#,##0.00",
    "dollars": "$#,##0",
    "thousands": "#,##0",
    "integer": "0",
    "decimal": "#,##0.00",
    "date": "yyyy-mm-dd",
}


def _sheet(rows, column_styles):
    out = []
    widths = {}
    for r, row in enumerate(rows, start=1):
        cells = []
        for c, value in enumerate(row):
            ref = f"{_column(c)}{r}"
            shown = "" if value is None else str(value)
            if isinstance(value, float):
                shown = f"{value:,.2f}"
            widths[c] = max(widths.get(c, 0), min(len(shown), 60))
            style = 1 if r == 1 else column_styles.get(c, 0)
            s = f' s="{style}"' if style else ""
            if isinstance(value, bool):
                cells.append(f'<c r="{ref}" t="b"{s}><v>{int(value)}</v></c>')
            elif isinstance(value, (int, float)):
                cells.append(f'<c r="{ref}"{s}><v>{value}</v></c>')
            elif isinstance(value, (datetime.date, datetime.datetime)):
                base = datetime.datetime(1899, 12, 30)
                moment = value if isinstance(value, datetime.datetime) else datetime.datetime(value.year, value.month, value.day)
                serial = (moment - base).total_seconds() / 86400
                cells.append(f'<c r="{ref}"{s}><v>{serial}</v></c>')
            elif value is None or value == "":
                continue
            else:
                cells.append(f'<c r="{ref}" t="inlineStr"{s}><is><t xml:space="preserve">{_text(value)}</t></is></c>')
        out.append(f'<row r="{r}">{"".join(cells)}</row>')
    columns = max((len(r) for r in rows), default=0)
    cols = "".join(
        f'<col min="{c + 1}" max="{c + 1}" width="{max(8, widths.get(c, 0) + 3)}" customWidth="1"/>'
        for c in range(columns)
    )
    view = (
        '<sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews>'
        if len(rows) > 1
        else ""
    )
    filters = f'<autoFilter ref="A1:{_column(columns - 1)}{len(rows)}"/>' if len(rows) > 1 and columns else ""
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
        f'{view}{"<cols>" + cols + "</cols>" if cols else ""}<sheetData>{"".join(out)}</sheetData>{filters}</worksheet>'
    )


def write_xlsx(path, rows, sheet="Sheet1", formats=None):
    """Write an .xlsx file. `rows` is a list of rows, or a dict of sheet name to rows."""
    path = _extension(path, "xlsx")
    sheets = rows if isinstance(rows, dict) else {sheet: rows}
    names = [re.sub(r"[\[\]:*?/\\]", "_", str(n))[:31] or "Sheet" for n in sheets]
    # Number formats: one cell style per distinct format, after the default (0) and header (1).
    codes = []
    per_sheet = []
    for rows_ in sheets.values():
        header = [str(h) for h in (rows_[0] if rows_ else [])]
        styles = {}
        for key, fmt in (formats or {}).items():
            code = _SHORTCUTS.get(str(fmt).lower(), str(fmt))
            if isinstance(key, int):
                col = key
            elif str(key) in header:
                col = header.index(str(key))
            elif re.fullmatch(r"[A-Za-z]{1,3}", str(key)):
                col = 0
                for ch in str(key).upper():
                    col = col * 26 + ord(ch) - 64
                col -= 1
            else:
                continue
            if code not in codes:
                codes.append(code)
            styles[col] = 2 + codes.index(code)
        per_sheet.append(styles)
    num_fmts = "".join(f'<numFmt numFmtId="{164 + i}" formatCode="{_text(c)}"/>' for i, c in enumerate(codes))
    extra_xfs = "".join(
        f'<xf numFmtId="{164 + i}" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/>'
        for i in range(len(codes))
    )
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        overrides = "".join(
            f'<Override PartName="/xl/worksheets/sheet{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
            for i in range(1, len(names) + 1)
        )
        z.writestr(
            "[Content_Types].xml",
            f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="{_CT}">'
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
            '<Default Extension="xml" ContentType="application/xml"/>'
            '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
            '<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>'
            f"{overrides}</Types>",
        )
        z.writestr(
            "_rels/.rels",
            f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{_PKG}">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>'
            "</Relationships>",
        )
        sheet_list = "".join(
            f'<sheet name="{_text(n)}" sheetId="{i}" r:id="rId{i}"/>' for i, n in enumerate(names, start=1)
        )
        z.writestr(
            "xl/workbook.xml",
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            f'<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="{_R}">'
            f"<sheets>{sheet_list}</sheets></workbook>",
        )
        rels = "".join(
            f'<Relationship Id="rId{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{i}.xml"/>'
            for i in range(1, len(names) + 1)
        )
        n = len(names) + 1
        rels += f'<Relationship Id="rId{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>'
        z.writestr(
            "xl/_rels/workbook.xml.rels",
            f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{_PKG}">{rels}</Relationships>',
        )
        z.writestr(
            "xl/styles.xml",
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
            + (f'<numFmts count="{len(codes)}">{num_fmts}</numFmts>' if codes else "")
            + '<fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts>'
            '<fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill>'
            '<fill><patternFill patternType="solid"><fgColor rgb="FFDCE6F1"/><bgColor indexed="64"/></patternFill></fill></fills>'
            '<borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>'
            '<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>'
            f'<cellXfs count="{2 + len(codes)}"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>'
            '<xf numFmtId="0" fontId="1" fillId="2" borderId="0" xfId="0" applyFont="1" applyFill="1"/>'
            f"{extra_xfs}</cellXfs>"
            '<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>'
            "</styleSheet>",
        )
        for i, rows_ in enumerate(sheets.values(), start=1):
            z.writestr(f"xl/worksheets/sheet{i}.xml", _sheet(rows_, per_sheet[i - 1]))
    return path


# --- PDF ---------------------------------------------------------------------------------

# Helvetica widths (1/1000 em) for ASCII 32-126, from the standard font metrics.
_HELV = [278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278,
         556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
         1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778,
         667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
         333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556,
         556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584]


def _width(text, size, font):
    if font == "F4":
        return len(text) * 600 * size / 1000
    scale = 1.06 if font == "F2" else 1.0
    total = 0
    for ch in text:
        o = ord(ch)
        total += _HELV[o - 32] if 32 <= o <= 126 else 556
    return total * scale * size / 1000


def _pdf_text(text):
    data = text.encode("cp1252", errors="replace")
    return "".join(
        "\\" + chr(b) if chr(b) in "()\\" else (chr(b) if 32 <= b < 127 else "\\%03o" % b) for b in data
    )


def _spans(text):
    """Inline Markdown to (text, font) pieces."""
    out = []
    for part in re.split(r"(\*\*[^*]+\*\*|\*[^*]+\*|`[^`]+`)", text):
        if not part:
            continue
        if part.startswith("**") and part.endswith("**") and len(part) > 4:
            out.append((part[2:-2], "F2"))
        elif part.startswith("`") and part.endswith("`") and len(part) > 2:
            out.append((part[1:-1], "F4"))
        elif part.startswith("*") and part.endswith("*") and len(part) > 2:
            out.append((part[1:-1], "F3"))
        else:
            out.append((part, "F1"))
    return out


def _wrap(spans, size, width):
    """Lines of (text, font) words that fit `width`."""
    words = []
    for text, font in spans:
        for i, w in enumerate(re.split(r"(\s+)", text)):
            if w and not w.isspace():
                words.append((w, font))
    lines, line, used = [], [], 0.0
    space = _width(" ", size, "F1")
    for w, font in words:
        need = _width(w, size, font) + (space if line else 0)
        if line and used + need > width:
            lines.append(line)
            line, used = [], 0.0
            need = _width(w, size, font)
        line.append((w, font))
        used += need
    if line:
        lines.append(line)
    return lines or [[]]


class _Pages:
    W, H, M = 612, 792, 72

    def __init__(self):
        self.pages = []
        self.new()

    def new(self):
        self.ops = []
        self.pages.append(self.ops)
        self.y = self.H - self.M

    def need(self, height):
        if self.y - height < self.M:
            self.new()

    def line(self, words, size, x, gap=0.0):
        self.need(size * 1.4)
        self.y -= size * 1.25
        cx = x
        space = _width(" ", size, "F1")
        for i, (w, font) in enumerate(words):
            self.ops.append(f"BT /{font} {size} Tf {cx:.2f} {self.y:.2f} Td ({_pdf_text(w)}) Tj ET")
            cx += _width(w, size, font) + space
        self.y -= size * 0.2 + gap

    def text(self, spans, size, x=None, gap=6.0, first=None):
        x = self.M if x is None else x
        lines = _wrap(spans, size, self.W - self.M - x)
        for i, words in enumerate(lines):
            if i == 0 and first:
                self.need(size * 1.4)
                self.ops.append(f"BT /F1 {size} Tf {x - 14:.2f} {self.y - size * 1.25:.2f} Td ({_pdf_text(first)}) Tj ET")
            self.line(words, size, x, gap if i == len(lines) - 1 else 0.0)

    def rule(self, y, x1, x2, width=0.5):
        self.ops.append(f"{width} w 0.6 G {x1:.2f} {y:.2f} m {x2:.2f} {y:.2f} l S 0 G")

    def table(self, rows):
        columns = max(len(r) for r in rows)
        total = self.W - 2 * self.M
        width = total / columns
        size = 9.5
        for r, row in enumerate(rows):
            row = list(row) + [""] * (columns - len(row))
            cells = [_wrap([(str(c), "F2" if r == 0 else "F1")], size, width - 8) for c in row]
            height = max(len(c) for c in cells) * size * 1.3 + 6
            self.need(height)
            top = self.y
            if r == 0:
                self.ops.append(f"0.86 0.9 0.95 rg {self.M:.2f} {top - height:.2f} {total:.2f} {height:.2f} re f 0 g")
            for c, lines in enumerate(cells):
                y = top - 3
                for words in lines:
                    y -= size * 1.3
                    x = self.M + c * width + 4
                    space = _width(" ", size, "F1")
                    for w, font in words:
                        self.ops.append(f"BT /{font} {size} Tf {x:.2f} {y + 2:.2f} Td ({_pdf_text(w)}) Tj ET")
                        x += _width(w, size, font) + space
            self.ops.append(f"0.5 w 0.6 G {self.M:.2f} {top - height:.2f} {total:.2f} {height:.2f} re S 0 G")
            for c in range(1, columns):
                x = self.M + c * width
                self.ops.append(f"0.5 w 0.6 G {x:.2f} {top:.2f} m {x:.2f} {top - height:.2f} l S 0 G")
            self.y = top - height
        self.y -= 10


def write_pdf(path, markdown, title=None):
    """Write a PDF from simple Markdown (the same subset as write_docx). Returns the path."""
    path = _extension(path, "pdf")
    pages = _Pages()
    sizes = {1: 22, 2: 16, 3: 13}
    lines = str(markdown).splitlines()
    para, table, number, first_heading = [], [], 0, True

    def flush():
        nonlocal para, table
        if para:
            pages.text(_spans(" ".join(para)), 11)
            para = []
        if table:
            rows = [r for r in table if not re.fullmatch(r"\|?[\s:|-]+\|?", r.strip())]
            parsed = [[c.strip() for c in r.strip().strip("|").split("|")] for r in rows]
            if parsed:
                pages.table(parsed)
            table = []

    for line in lines:
        stripped = line.strip()
        heading = re.match(r"(#{1,6})\s+(.*)", stripped)
        bullet = re.match(r"[-*+]\s+(.*)", stripped)
        numbered = re.match(r"(\d+)[.)]\s+(.*)", stripped)
        if stripped.startswith("|"):
            if para:
                flush()
            table.append(stripped)
            continue
        if table:
            flush()
        if not stripped:
            flush()
            number = 0
        elif heading:
            flush()
            level = len(heading.group(1))
            size = sizes.get(min(level, 3), 13)
            pages.y -= 8 if not first_heading else 0
            pages.need(size * 3)
            pages.text([(heading.group(2).replace("**", ""), "F2")], size, gap=4)
            if level == 1 and first_heading:
                pages.rule(pages.y - 2, pages.M, pages.W - pages.M)
                pages.y -= 10
            first_heading = False
        elif bullet:
            flush()
            pages.text(_spans(bullet.group(1)), 11, x=pages.M + 18, gap=3, first="•")
        elif numbered:
            flush()
            number += 1
            pages.text(_spans(numbered.group(2)), 11, x=pages.M + 18, gap=3, first=f"{number}.")
        else:
            para.append(stripped)
    flush()

    objects = []

    def add(body):
        objects.append(body)
        return len(objects)

    fonts = {
        "F1": add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"),
        "F2": add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"),
        "F3": add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Oblique /Encoding /WinAnsiEncoding >>"),
        "F4": add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>"),
    }
    font_dict = " ".join(f"/{k} {v} 0 R" for k, v in fonts.items())
    pages_id = len(objects) + 1 + 2 * len(pages.pages) + 1
    kids = []
    for ops in pages.pages:
        stream = zlib.compress("\n".join(ops).encode("latin-1"))
        content = add(b"<< /Length " + str(len(stream)).encode() + b" /Filter /FlateDecode >>\nstream\n" + stream + b"\nendstream")
        kids.append(add(f"<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {_Pages.W} {_Pages.H}] /Resources << /Font << {font_dict} >> >> /Contents {content} 0 R >>".encode()))
    info = add(f"<< /Title ({_pdf_text(title or '')}) /Producer (Spindle) >>".encode())
    assert add(f"<< /Type /Pages /Kids [{' '.join(f'{k} 0 R' for k in kids)}] /Count {len(kids)} >>".encode()) == pages_id
    catalog = add(f"<< /Type /Catalog /Pages {pages_id} 0 R >>".encode())
    out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for i, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += f"{i} 0 obj\n".encode() + body + b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objects) + 1}\n0000000000 65535 f \n".encode()
    for o in offsets:
        out += f"{o:010d} 00000 n \n".encode()
    out += f"trailer\n<< /Size {len(objects) + 1} /Root {catalog} 0 R /Info {info} 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    with open(path, "wb") as f:
        f.write(bytes(out))
    return path
