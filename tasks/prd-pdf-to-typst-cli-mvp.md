# PRD: PDF to Typst CLI MVP

## Overview
개발자가 단일 PDF를 입력하면 출력 디렉터리에 컴파일 가능한 Typst 문서와 관련 자산을 생성하는 CLI를 제공한다. MVP는 디지털 PDF뿐 아니라 스캔/OCR PDF도 지원하며, 한국어/영어 혼합 문서를 1차 타깃으로 삼는다. 핵심 가치는 수작업 재작성 비용을 줄이면서 텍스트뿐 아니라 표, 이미지, 캡션까지 가능한 한 Typst 구조로 복원하는 것이다.

## Goals
- 단일 PDF를 출력 디렉터리 기반 Typst 결과물로 변환한다.
- 생성된 주 `.typ` 파일이 바로 Typst 컴파일 가능해야 한다.
- 디지털 PDF와 스캔/OCR PDF를 모두 지원한다.
- 표, 이미지, 캡션을 가능한 한 구조적으로 보존한다.
- 지원 불가 요소에 대해 경고 기반 계속 진행과 엄격 실패 모드를 모두 제공한다.

## Quality Gates
These commands must pass for every user story:
- `cargo fmt --check`
- `cargo test`

UI/browser verification is not applicable for this CLI PRD.

## User Stories

### US-001: Define CLI contract and output layout
**Description:** As a developer, I want to convert one PDF into a predictable output directory so that I can inspect and compile the generated Typst files.

**Acceptance Criteria:**
- [ ] CLI는 입력 PDF 경로와 출력 디렉터리 경로를 받는다.
- [ ] 성공 시 출력 디렉터리에 하나의 주 `.typ` 파일과 참조되는 자산 파일들을 생성한다.
- [ ] 도움말에 필수 인자와 주요 옵션(`--strict` 포함)이 문서화된다.
- [ ] 성공, 경고 동반 성공, 치명적 실패에 대한 동작이 문서화되고 테스트로 검증된다.

### US-002: Emit Typst from digital PDFs
**Description:** As a developer, I want text-based PDFs to be converted into valid Typst structure so that I can reuse existing digital documents without manual rewriting.

**Acceptance Criteria:**
- [ ] 디지털 PDF의 텍스트가 읽기 순서대로 추출된다.
- [ ] 제목, 문단, 목록, 코드 유사 블록 등 감지 가능한 구조가 유효한 Typst 문법으로 매핑된다.
- [ ] 다중 페이지 입력에서도 문서 순서가 보존된다.
- [ ] 지원하지 못한 구조는 조용히 누락되지 않고 진단 정보로 드러난다.

### US-003: Add OCR support for scanned Korean/English PDFs
**Description:** As a developer, I want scanned PDFs to go through OCR so that Korean/English mixed documents can also be converted into Typst.

**Acceptance Criteria:**
- [ ] 이미지 기반 스캔 PDF를 OCR 경로로 처리할 수 있다.
- [ ] 기본 OCR 설정은 한국어/영어 혼합 문서를 우선 대상으로 삼는다.
- [ ] OCR 결과 텍스트가 디지털 PDF와 동일한 후속 Typst 생성 흐름으로 연결된다.
- [ ] OCR 사용 불가 또는 낮은 신뢰도 상황에서 원인과 영향을 알 수 있는 진단이 제공된다.

### US-004: Convert tables, images, and captions
**Description:** As a developer, I want non-text elements preserved in Typst so that converted documents remain useful beyond plain text extraction.

**Acceptance Criteria:**
- [ ] 감지된 이미지는 재사용 가능한 자산 파일로 추출되고 생성된 Typst에서 참조된다.
- [ ] 구조 신뢰도가 충분한 표는 Typst 표 요소로 출력된다.
- [ ] 이미지와 표의 캡션은 감지 가능한 경우 보존된다.
- [ ] 풍부한 요소 변환이 불완전하거나 불가능한 경우 어떤 요소가 저하되었는지 기록된다.

### US-005: Support strict and non-strict conversion modes
**Description:** As a developer, I want to choose between best-effort conversion and fail-fast validation so that I can use the CLI for both exploration and strict pipelines.

**Acceptance Criteria:**
- [ ] 기본 모드는 지원 불가 요소가 있어도 가능한 범위까지 변환을 계속하고 경고를 남긴다.
- [ ] `--strict` 모드는 지원 불가 또는 불완전 변환을 경고가 아닌 실패로 처리한다.
- [ ] 진단 정보는 가능할 때 페이지 또는 요소 수준의 문맥을 포함한다.
- [ ] 동일한 실패 사례에 대해 기본 모드와 `--strict` 모드의 차이가 테스트로 검증된다.

### US-006: Validate compileability and regression coverage
**Description:** As a developer, I want automated regression checks on sample PDFs so that converted Typst output stays compileable as the converter evolves.

**Acceptance Criteria:**
- [ ] 저장소의 `data/` 샘플 PDF들을 자동 테스트에 사용할 수 있다.
- [ ] `cargo test` 안에서 지원 대상 샘플의 생성 `.typ`가 실제로 컴파일 가능한지 검증한다.
- [ ] 회귀 테스트는 최소 1개의 디지털 PDF와 1개의 스캔 한국어/영어 혼합 PDF를 포함한다.
- [ ] 테스트 실패 시 어떤 샘플과 어떤 변환 단계가 깨졌는지 식별 가능하다.

## Functional Requirements
1. FR-1: 시스템은 단일 입력 PDF와 출력 디렉터리를 받아야 한다.
2. FR-2: 시스템은 결정적인 이름 규칙으로 주 `.typ` 파일과 자산 파일을 생성해야 한다.
3. FR-3: 시스템은 디지털 텍스트 기반 PDF를 처리해야 한다.
4. FR-4: 시스템은 스캔 PDF에 대해 OCR 경로를 제공해야 하며, 기본 대상 언어는 한국어와 영어여야 한다.
5. FR-5: 시스템은 텍스트를 읽기 순서에 맞춰 재구성하고 감지 가능한 문서 구조를 Typst 문법으로 변환해야 한다.
6. FR-6: 시스템은 이미지, 표, 캡션을 가능한 한 Typst 표현과 자산 파일로 보존해야 한다.
7. FR-7: 시스템은 지원 불가 요소를 만났을 때 경고 기반 계속 진행과 `--strict` 실패 모드를 모두 제공해야 한다.
8. FR-8: 시스템은 경고 또는 실패 사유를 사용자에게 이해 가능한 진단 형태로 출력해야 한다.
9. FR-9: 시스템은 지원 대상 입력에 대해 컴파일 가능한 Typst 결과물을 생성해야 한다.
10. FR-10: 시스템은 `data/` 기반 회귀 테스트를 통해 변환 품질과 컴파일 가능성을 검증해야 한다.

## Non-Goals
이번 라운드에서는 명시적 비목표를 두지 않는다. 스캔/OCR 지원, 표/이미지/캡션 보존, 엄격 모드 모두 MVP 범위에 포함한다.

## Technical Considerations
- 현재 저장소에는 `data/` 샘플 PDF만 존재하므로, 구현 단계에서 Rust CLI 프로젝트 구조와 테스트 하네스를 함께 정리해야 할 가능성이 높다.
- OCR 엔진은 한국어/영어 혼합 문서를 안정적으로 다룰 수 있어야 하며, 로컬과 CI에서 재현 가능해야 한다.
- 변환 파이프라인은 PDF 추출/OCR, 문서 구조 복원, Typst 출력 단계를 분리하는 편이 유지보수에 유리하다.
- Typst 컴파일 검증을 자동화하려면 테스트 환경에서 Typst 실행 가능성을 확보해야 한다.
- 자산 파일 이름 규칙과 경고 출력 형식은 회귀 테스트와 디버깅을 위해 결정적이어야 한다.

## Success Metrics
- 지원 대상 샘플 PDF의 100%가 주 `.typ` 파일 생성까지 완료된다.
- 지원 대상 샘플 PDF의 100%가 자동 테스트에서 Typst 컴파일에 성공한다.
- 지원 불가 요소에 대한 무음 누락이 0건이어야 하며, 모든 저하 변환은 경고 또는 엄격 실패로 드러난다.
- 회귀 테스트 세트에 디지털 PDF와 스캔 한국어/영어 혼합 PDF가 모두 포함된다.

## Open Questions
- 한국어/영어 혼합 문서에 대한 기본 OCR 엔진 또는 라이브러리는 무엇으로 할 것인가?
- 경고는 표준 출력/표준 에러만으로 충분한가, 아니면 머신 리더블 리포트 파일도 함께 생성해야 하는가?
- 생성되는 Typst 결과물은 어떤 기본 템플릿 또는 프로젝트 스캐폴딩을 전제로 해야 하는가?
- 표 구조 신뢰도가 낮을 때 기본 폴백은 텍스트 선형화가 좋은가, 이미지 보존이 좋은가?
- Typst 컴파일 검증을 로컬 개발 환경과 CI에서 어떤 방식으로 일관되게 보장할 것인가?