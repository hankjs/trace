<script setup lang="ts" generic="Row extends object">
import type { QuTableColumn, QuTableRowClass, QuTableRowKey } from './quTable'

const props = withDefaults(defineProps<{
  data: readonly Row[]
  columns: readonly QuTableColumn<Row>[]
  rowKey?: QuTableRowKey<Row>
  headClass?: string
  headerRowClass?: string
  headerCellClass?: string
  bodyRowClass?: QuTableRowClass<Row>
  bodyCellClass?: string
}>(), {
  headClass: '',
  headerRowClass: 'border-b border-border text-left text-xs text-text-tertiary',
  headerCellClass: 'px-4 py-2 font-medium',
  bodyRowClass: 'border-b border-border-subtle last:border-0 hover:bg-hover',
  bodyCellClass: 'px-4 py-2',
})
const emit = defineEmits<{
  rowClick: [row: Row, rowIndex: number, event: MouseEvent]
}>()

function valueOf(row: Row, column: QuTableColumn<Row>, rowIndex: number): unknown {
  if (column.value) return column.value(row, rowIndex)
  return (row as Record<string, unknown>)[column.key]
}

function displayValue(row: Row, column: QuTableColumn<Row>, rowIndex: number): string | number {
  const value = valueOf(row, column, rowIndex)
  if (column.format) return column.format(value, row, rowIndex)
  if (value === null || value === undefined) return '--'
  return typeof value === 'string' || typeof value === 'number' ? value : String(value)
}

function keyOf(row: Row, rowIndex: number): PropertyKey {
  if (typeof props.rowKey === 'function') return props.rowKey(row, rowIndex)
  if (props.rowKey !== undefined) {
    const value = (row as Record<string, unknown>)[props.rowKey]
    if (typeof value === 'string' || typeof value === 'number' || typeof value === 'symbol') return value
  }

  const record = row as Record<string, unknown>
  for (const candidate of ['id', 'code', 'key']) {
    const value = record[candidate]
    if (typeof value === 'string' || typeof value === 'number' || typeof value === 'symbol') return value
  }
  return rowIndex
}

function alignClass(column: QuTableColumn<Row>): string {
  if (column.align === 'right') return 'text-right'
  if (column.align === 'center') return 'text-center'
  return 'text-left'
}

function cellClass(column: QuTableColumn<Row>, row: Row, rowIndex: number): string {
  return typeof column.cellClass === 'function'
    ? column.cellClass(row, rowIndex)
    : column.cellClass ?? ''
}

function rowClass(row: Row, rowIndex: number): string {
  return typeof props.bodyRowClass === 'function'
    ? props.bodyRowClass(row, rowIndex)
    : props.bodyRowClass
}

function hasColumnWidths(): boolean {
  return props.columns.some((column) => Boolean(column.widthClass))
}
</script>

<template>
  <table class="w-full text-sm" :aria-rowcount="data.length">
    <colgroup v-if="hasColumnWidths()">
      <col v-for="column in columns" :key="column.key" :class="column.widthClass" />
    </colgroup>
    <thead :class="headClass">
      <tr :class="headerRowClass">
        <th
          v-for="column in columns"
          :key="column.key"
          scope="col"
          :class="[headerCellClass, alignClass(column), column.headerClass]"
        >
          <slot :name="`header-${column.key}`" :column="column">
            {{ column.label }}
          </slot>
        </th>
      </tr>
    </thead>
    <tbody>
      <template v-for="(row, rowIndex) in data" :key="keyOf(row, rowIndex)">
        <tr :class="rowClass(row, rowIndex)" @click="emit('rowClick', row, rowIndex, $event)">
          <td
            v-for="column in columns"
            :key="column.key"
            :class="[bodyCellClass, alignClass(column), cellClass(column, row, rowIndex)]"
          >
            <slot
              :name="`cell-${column.key}`"
              :row="row"
              :column="column"
              :value="valueOf(row, column, rowIndex)"
              :row-index="rowIndex"
            >
              <slot
                name="cell"
                :row="row"
                :column="column"
                :value="valueOf(row, column, rowIndex)"
                :row-index="rowIndex"
              >
                {{ displayValue(row, column, rowIndex) }}
              </slot>
            </slot>
          </td>
        </tr>
        <slot name="after-row" :row="row" :row-index="rowIndex" :colspan="columns.length" />
      </template>
    </tbody>
  </table>
</template>
