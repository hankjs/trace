export interface ResearchGuide {
  title: string
  summary: string
  concepts?: { term: string; explanation: string }[]
  steps?: string[]
  note?: string
}
