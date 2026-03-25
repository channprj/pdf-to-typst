# PRD: `kr-book-01.pdf` 검증 기준의 pdf-to-typst 변환 품질 고도화

## Overview
`pdf-to-typst`를 범용적으로 개선하되, `./data/kr-book-01.pdf`를 대표 검증 샘플로 사용한다. 시스템은 문서 첫 3페이지에서 네이티브 텍스트 추출과 OCR 결과를 비교해 문서 전체 기본 전략을 정하고, 일반 본문·캡션·단순 표는 editable Typst 텍스트/구조로 유지한다. 복잡한 표·도해·캡션 군집만 요소 단위 이미지로 폴백하며, whole-page raster fallback은 금지한다. 반복 검증은 CLI 기능이 아니라 ralph loop 세션에서 같은 샘플 파일을 계속 재실행하며 품질이 높아질 때까지 수행한다.

## Goals
- 첫 3페이지 샘플링만으로 OCR vs native 추출 기본 전략을 결정한다.
- 본문과 캡션은 가능한 한 editable/searchable Typst text로 유지한다.
- 복잡한 표/도해만 요소 단위 이미지로 폴백하고, 페이지 전체 이미지는 사용하지 않는다.
- 생성된 `main.typ`가 `typst compile`을 통과한다.
- `kr-book-01.pdf` 첫 3페이지는 텍스트 유지 + 느슨한 렌더 유사도 기준을 모두 만족한다.
- 전체 문서 렌더 비교를 시도하되, 첫 3페이지를 필수 샘플 게이트로 사용한다.
- 사용자-facing CLI 옵션은 추가하지 않는다.

## Quality Gates
These commands must pass for every user story:
- `cargo fmt --check`
- `cargo clippy -- -D warnings`
- `cargo test`
- `cargo run -- ./data/kr-book-01.pdf /tmp/pdf-to-typst-kr-book-01-out --strict`
- `typst compile /tmp/pdf-to-typst-kr-book-01-out/main.typ`
- `--strict` 실행 전에는 항상 비어 있는 출력 디렉터리를 사용한다.
- 품질 검증은 ralph loop 세션에서 수행한다: 원본 PDF와 생성된 Typst 렌더를 전체 문서 기준으로 비교 시도하고, 첫 3페이지는 반드시 재검증하며 고품질 상태가 나올 때까지 수정 후 재실행한다.

## User Stories

### US-001: 3페이지 샘플 기반 OCR 전략 결정
**Description:** As a maintainer, I want the converter to compare native extraction and OCR on the first three pages so that it chooses a better default strategy without paying full-document OCR cost.

**Acceptance Criteria:**
- [ ] 동일 입력에 대해 페이지 1-3만 비교 샘플로 사용해 문서 기본 전략을 결정한다.
- [ ] 문서가 3페이지 미만이면 존재하는 페이지만 대상으로 한다.
- [ ] 동일 입력을 반복 실행했을 때 기본 전략 선택은 결정적이다.
- [ ] 샘플 페이지 중 OCR 후보가 없거나 지원되지 않아도 변환은 경고와 함께 계속된다.

### US-002: 텍스트 우선 Typst 재구성
**Description:** As a reader, I want normal text content to remain editable Typst text so that the output is searchable and reusable while staying close to the source PDF.

**Acceptance Criteria:**
- [ ] `kr-book-01.pdf` 첫 3페이지의 본문 텍스트는 생성된 `main.typ` 안에 Typst 텍스트 리터럴로 존재한다.
- [ ] 첫 3페이지의 기본 읽기 순서는 위에서 아래, 좌에서 우 기준으로 크게 깨지지 않는다.
- [ ] 본문, 단순 캡션, 구조적으로 단순한 텍스트 영역은 이미지 자산으로 대체되지 않는다.
- [ ] 추출 가능한 한국어 텍스트는 검색 가능하고 편집 가능한 형태로 남는다.

### US-003: 복잡 요소만 이미지 폴백
**Description:** As a reader, I want only structurally complex tables and figures to fall back to images so that difficult regions stay legible without flattening the entire page.

**Acceptance Criteria:**
- [ ] 구조 추출 실패, 병합 셀, 회전 구조, 중첩 캡션 군집 등 복잡성 신호가 있는 경우에만 요소 단위 이미지 폴백을 사용한다.
- [ ] 단순 표와 일반 이미지 배치는 가능한 한 구조화된 Typst 출력 경로를 유지한다.
- [ ] 한 페이지의 일부 요소가 이미지로 폴백되어도 같은 페이지의 unrelated text는 계속 텍스트로 유지된다.
- [ ] 어떤 페이지에도 whole-page raster fallback은 생성되지 않는다.

### US-004: 컴파일 가능하고 비교 가능한 출력 유지
**Description:** As a maintainer, I want the generated Typst to compile and roughly match the PDF layout so that render comparison highlights only meaningful regressions.

**Acceptance Criteria:**
- [ ] `cargo run -- ./data/kr-book-01.pdf ... --strict` 실행 결과로 생성된 프로젝트는 추가 수동 수정 없이 `typst compile`이 성공한다.
- [ ] 문단, 표, 이미지, 캡션의 대략적 위치 관계가 원본과 크게 어긋나지 않는다.
- [ ] 렌더 비교는 전체 문서에 대해 시도하고, 첫 3페이지는 필수 통과 샘플로 취급한다.
- [ ] 자간·커닝·미세 간격 차이는 허용하지만, 누락·중복·심한 오배치·가독성 붕괴는 실패로 본다.

### US-005: 세션 주도 반복 검증
**Description:** As an operator, I want the ralph loop session to rerun the same sample document after each fix so that converter quality improves iteratively without embedding a retry loop into the CLI.

**Acceptance Criteria:**
- [ ] `pdf-to-typst` 바이너리 자체에는 내부 재시도/자가반복 기능을 추가하지 않는다.
- [ ] 반복 검증은 항상 같은 샘플 파일 `./data/kr-book-01.pdf`로 수행한다.
- [ ] 코드 수정 후에는 변환과 `typst compile`을 다시 실행한다.
- [ ] 작업 완료 판정은 첫 3페이지가 텍스트 유지와 느슨한 렌더 유사도 기준을 만족하고, 나머지 페이지에 고심각도 가독성 실패가 없을 때만 내려진다.

## Functional Requirements
1. FR-1: 시스템은 입력 문서의 첫 3페이지에서 네이티브 텍스트 추출과 OCR 결과를 모두 평가해야 한다.
2. FR-2: 시스템은 첫 3페이지 평가 결과를 바탕으로 문서 전체 기본 추출 전략을 결정해야 한다.
3. FR-3: 시스템은 본문, 제목, 단순 캡션, 일반 텍스트 영역을 우선적으로 Typst 텍스트로 출력해야 한다.
4. FR-4: 시스템은 복잡한 표·도해·캡션 군집에 대해서만 요소 단위 이미지 폴백을 허용해야 한다.
5. FR-5: 시스템은 어떤 경우에도 페이지 전체를 단일 이미지로 내보내는 fallback을 사용해서는 안 된다.
6. FR-6: 시스템은 OCR 후보 부족, 지원되지 않는 이미지 인코딩, 부분 추출 실패 시 경고를 내고 가능한 경로로 계속 진행해야 한다.
7. FR-7: 시스템은 사용자-facing CLI 옵션이나 인자를 추가하지 않아야 한다.
8. FR-8: 시스템은 `./data/kr-book-01.pdf`에 대해 `--strict` 모드와 `typst compile`을 통과하는 산출물을 생성해야 한다.
9. FR-9: 시스템은 전체 문서 비교를 가능하게 할 정도로 안정적이고 재현 가능한 출력 구조를 유지해야 한다.
10. FR-10: 시스템은 full-document dual OCR 비용을 피하기 위해 이중 평가 범위를 첫 3페이지로 제한해야 한다.

## Non-Goals
- 새 CLI 옵션 추가
- `pdf-to-typst` 내부에 자동 재시도 루프, self-healing 루프, 세션 오케스트레이션 기능 추가
- 픽셀 단위의 완전 동일 렌더 보장
- whole-page raster fallback 유지 또는 확장
- 샘플 PDF에만 맞춘 수동 하드코딩
- 사람이 페이지별로 손으로 Typst를 교정하는 운영 절차를 제품 기능으로 포함

## Technical Considerations
- 주요 변경 지점은 [src/lib.rs](/Volumes/990EVO+/workspace/pdf-to-typst/src/lib.rs) 의 변환 파이프라인, OCR 처리, PDFKit 경로, 기존 raster fallback 경로일 가능성이 높다.
- CLI surface는 [src/main.rs](/Volumes/990EVO+/workspace/pdf-to-typst/src/main.rs) 와 [src/lib.rs](/Volumes/990EVO+/workspace/pdf-to-typst/src/lib.rs) 의 인자 파싱 기준에서 유지되어야 한다.
- 현재 문서화된 raster fallback 동작은 [README.md](/Volumes/990EVO+/workspace/pdf-to-typst/README.md) 와 실제 구현이 어긋나지 않도록 후속 정리가 필요하다.
- `kr-book-01.pdf`는 한국어 문서이므로 텍스트 리터럴 처리, 줄바꿈, 글꼴 매핑, 읽기 순서 안정성이 핵심이다.
- 반복 검증 보고서는 제품 기능이 아니라 ralph session 운영 산출물로 취급한다.

## Success Metrics
- 모든 Quality Gates가 통과한다.
- `kr-book-01.pdf` 첫 3페이지에서 whole-page raster fallback 사용 건수는 0이다.
- `kr-book-01.pdf` 첫 3페이지에서 추출 가능한 본문 텍스트가 이미지로 대체되지 않는다.
- 전체 문서 렌더 비교에서 고심각도 가독성 실패 페이지 수는 0이다.
- 첫 3페이지는 텍스트 유지와 느슨한 시각 유사도 기준을 동시에 만족한다.
- 이중 평가가 첫 3페이지로 제한되어 전체 문서 처리 시간이 불필요하게 급증하지 않는다.

## Open Questions
- 네이티브 추출과 OCR 중 “우승 전략”을 결정하는 구체 점수식은 텍스트 커버리지, 신뢰도, 기하 배치 안정성 중 무엇을 얼마나 반영할지 구현 단계에서 확정이 필요하다.
- 기존 whole-page raster 관련 회귀 테스트와 문서를 완전히 제거할지, 내부 비활성 경로로 남길지는 구현 중 판단이 필요하다.