export type QuTableAlign = 'left' | 'center' | 'right'

export interface QuTableColumn<Row extends object = Record<string, unknown>> {
  key: string
  label: string
  align?: QuTableAlign
  widthClass?: string
  headerClass?: string
  cellClass?: string | ((row: Row, rowIndex: number) => string)
  value?: (row: Row, rowIndex: number) => unknown
  format?: (value: unknown, row: Row, rowIndex: number) => string | number
}

export type QuTableRowKey<Row extends object> = string | ((row: Row, rowIndex: number) => PropertyKey)
export type QuTableRowClass<Row extends object> = string | ((row: Row, rowIndex: number) => string)
