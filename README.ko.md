# pdf-to-typst

[English](README.md) | **한국어**

> PDF 문서를 편집 가능한 Typst 프로젝트로 변환합니다.

`pdf-to-typst`는 단일 PDF를 입력받아 바로 컴파일 가능한 [Typst](https://typst.app/) 프로젝트 — `main.typ`과 `assets/` 디렉토리 — 를 생성하며, 텍스트, 이미지, 표, 레이아웃을 최대한 원본에 가깝게 보존합니다.

## 주요 기능

- 네이티브 PDF 텍스트 추출 및 구조 분석
- 스캔 문서 OCR (한국어 + 영어 기본 지원)
- 표, 이미지, 캡션 보존
- PDFKit 기반 텍스트 위치 복원 (macOS)
- Ghostscript 래스터 폴백 (복잡한 페이지 처리)
- CI/파이프라인용 `--strict` 모드 (경고를 오류로 승격)

## 빠른 시작

### macOS (Homebrew)

```sh
brew install channprj/tap/pdf-to-typst
brew install channprj/tap/pdf-to-typst@0.260323.2
```

> **참고:** tap은 각 릴리즈의 태그 소스 아카이브에서 빌드되며, helper script는
> Homebrew의 `lib/pdf-to-typst/tools` 경로에 배치됩니다.

### GitHub Releases에서 다운로드

[Releases](https://github.com/channprj/pdf-to-typst/releases)에는 `main`에
머지된 커밋 메시지에 `release`가 포함되거나, 릴리즈 워크플로를 수동 실행했을 때
미리 빌드된 아카이브가 올라갑니다. 릴리즈 버전은 저장소 루트의 `VERSION`
파일에서 직접 읽으며, 예시는 `v0.260323.2`입니다. 이 값은 Git 태그,
GitHub Release, `pdf-to-typst --version` 출력, `channprj/tap` Homebrew
포뮬러에 동일하게 사용됩니다.
다음 대상의 바이너리가 게시됩니다.

| 플랫폼 | Target |
|--------|--------|
| macOS Apple Silicon | `aarch64-apple-darwin` |
| macOS Intel | `x86_64-apple-darwin` |
| Linux x86_64 | `x86_64-unknown-linux-gnu` |

```sh
tar xzf pdf-to-typst-v*.tar.gz
cd pdf-to-typst-v*
./pdf-to-typst --version
./pdf-to-typst input.pdf output/
```

### 소스에서 빌드

```sh
cargo install --git https://github.com/channprj/pdf-to-typst
```

또는 직접 클론하여 빌드:

```sh
git clone https://github.com/channprj/pdf-to-typst.git
cd pdf-to-typst
cargo build --release
# 바이너리 위치: target/release/pdf-to-typst
```

## 의존성

### 필수

- **Ghostscript** (`gs`) — 복잡한 PDF 페이지의 래스터 폴백에 사용

### 선택

- **Tesseract** — 스캔 페이지 OCR (기본 언어: `kor+eng`)
- **Python 3 + ImageMagick** (`convert`) — 비텍스트 영역 추출
- **Xcode Command Line Tools** — PDFKit 텍스트 위치 복원 (macOS 전용)
- **Typst** — 생성된 프로젝트를 바로 컴파일하려는 경우에만 필요

### 플랫폼별 설치 명령

**macOS (Homebrew):**

```sh
brew install ghostscript tesseract imagemagick typst
# Xcode CLI Tools: xcode-select --install
```

**Ubuntu / Debian:**

```sh
sudo apt-get install ghostscript tesseract-ocr imagemagick
```

**Fedora:**

```sh
sudo dnf install ghostscript tesseract ImageMagick
```

## 사용법

### 기본 사용

```sh
pdf-to-typst input.pdf output/
```

성공 시 생성된 `main.typ` 경로를 출력하고 종료 코드 `0`으로 종료합니다.

### Strict 모드

```sh
pdf-to-typst input.pdf output/ --strict
```

Strict 모드에서는 모든 경고(예: 비어있지 않은 출력 디렉토리 재사용)가 치명적 오류(종료 코드 `2`)로 승격됩니다.

### 환경 변수

| 변수 | 설명 |
|------|------|
| `PDF_TO_TYPST_TOOLS_DIR` | `tools/` 헬퍼 스크립트 경로 직접 지정 |

### 출력 구조

```
output/
├── main.typ      # Typst 메인 진입점
└── assets/       # 추출된 이미지 및 리소스
```

### 종료 코드

| 코드 | 의미 |
|------|------|
| `0`  | 성공 (경고가 stderr에 출력될 수 있음) |
| `2`  | 치명적 오류 — 출력 파일 미생성 |

## 동작 원리

`pdf-to-typst`는 PDF 내용에 따라 세 가지 변환 경로를 사용합니다:

1. **네이티브 텍스트 추출** — PDF 바이너리 구조를 직접 파싱하여 텍스트, 폰트, 레이아웃을 추출합니다. 디지털 생성 PDF의 주요 처리 경로입니다.

2. **PDFKit 씬 분석** (macOS) — Apple의 PDFKit 프레임워크를 Swift 헬퍼를 통해 활용하여 정밀한 텍스트 위치를 복원하고 페이지를 렌더링합니다. 복잡한 레이아웃의 충실도를 높입니다.

3. **OCR 폴백** — 텍스트가 임베드되지 않은 스캔 문서의 경우 Tesseract로 광학 문자 인식을 수행합니다. 기본적으로 한국어와 영어를 지원합니다.

네이티브 파싱으로 복잡한 페이지를 안전하게 재구성할 수 없는 경우, 페이지별 래스터 이미지로 폴백하여 생성된 Typst 프로젝트의 미리보기와 내보내기가 가능하도록 합니다.

## 문제 해결

### `error: gs not found`

Ghostscript가 필요합니다. 패키지 매니저로 설치하세요 ([의존성](#의존성) 참조).

### `warning: tesseract not available`

스캔 페이지의 OCR이 건너뛰어집니다. Tesseract를 설치하면 OCR 기능을 사용할 수 있습니다.

### `warning: reusing non-empty output directory`

출력 디렉토리에 이미 파일이 존재합니다. 기본 모드에서는 경고이며, `--strict` 모드에서는 치명적 오류입니다. 빈 디렉토리를 사용하거나 기존 파일을 삭제하세요.

### PDFKit 헬퍼 컴파일 실패

Xcode Command Line Tools가 설치되어 있는지 확인하세요: `xcode-select --install`. 이 기능은 macOS 전용입니다.

## 기여하기

기여를 환영합니다! [GitHub](https://github.com/channprj/pdf-to-typst)에서 이슈를 열거나 풀 리퀘스트를 제출해 주세요.

## 라이선스

[MIT](LICENSE)
