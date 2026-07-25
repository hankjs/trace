import { computed, reactive, toValue, type MaybeRefOrGetter } from 'vue'
import type { CatalogParameter, StrategyParamValue } from './api'

type ParamValues = Record<string, StrategyParamValue>

function parameterType(parameter: CatalogParameter): NonNullable<CatalogParameter['value_type']> {
  if (parameter.value_type) return parameter.value_type
  if (typeof parameter.default === 'boolean') return 'boolean'
  if (typeof parameter.default === 'string') return 'string'
  return 'number'
}

export function useStrategyParamForm(
  parameters: MaybeRefOrGetter<readonly CatalogParameter[]>,
  initialValues?: MaybeRefOrGetter<ParamValues | null | undefined>
) {
  const values = reactive<ParamValues>({})
  const errors = reactive<Record<string, string>>({})

  function clear(target: Record<string, unknown>) {
    for (const key of Object.keys(target)) delete target[key]
  }

  function reset(source: ParamValues = toValue(initialValues) ?? {}) {
    clear(values)
    clear(errors)
    for (const parameter of toValue(parameters)) {
      const value = source[parameter.key] ?? parameter.default
      if (value !== undefined) values[parameter.key] = value
    }
  }

  function validate(): boolean {
    clear(errors)
    for (const parameter of toValue(parameters)) {
      const value = values[parameter.key]
      const type = parameterType(parameter)
      if (value === undefined || value === '') {
        errors[parameter.key] = `请填写${parameter.name}`
        continue
      }
      if (type === 'boolean') {
        if (typeof value !== 'boolean') errors[parameter.key] = `${parameter.name}必须为布尔值`
        continue
      }
      if (type === 'string') {
        if (typeof value !== 'string') errors[parameter.key] = `${parameter.name}必须为文本`
        continue
      }
      if (typeof value !== 'number' || !Number.isFinite(value)) {
        errors[parameter.key] = `${parameter.name}必须为有效数字`
        continue
      }
      if (type === 'integer' && !Number.isInteger(value)) {
        errors[parameter.key] = `${parameter.name}必须为整数`
      } else if (parameter.minimum !== undefined && value < parameter.minimum) {
        errors[parameter.key] = `${parameter.name}不能小于 ${parameter.minimum}`
      } else if (parameter.maximum !== undefined && value > parameter.maximum) {
        errors[parameter.key] = `${parameter.name}不能大于 ${parameter.maximum}`
      }
    }
    return Object.keys(errors).length === 0
  }

  function snapshot(): ParamValues {
    return Object.fromEntries(
      toValue(parameters)
        .filter((parameter) => values[parameter.key] !== undefined && values[parameter.key] !== '')
        .map((parameter) => [parameter.key, values[parameter.key]])
    )
  }

  const overrides = computed<ParamValues>(() => {
    const result: ParamValues = {}
    for (const parameter of toValue(parameters)) {
      const value = values[parameter.key]
      if (value === undefined || value === '' || value === parameter.default) continue
      result[parameter.key] = value
    }
    return result
  })

  function differsFrom(source: ParamValues): boolean {
    return toValue(parameters).some((parameter) => {
      const baseline = source[parameter.key] ?? parameter.default
      return values[parameter.key] !== baseline
    })
  }

  return {
    values,
    errors,
    overrides,
    reset,
    validate,
    snapshot,
    differsFrom,
  }
}
