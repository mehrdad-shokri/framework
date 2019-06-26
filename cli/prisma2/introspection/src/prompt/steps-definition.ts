import { DatabaseType } from 'prisma-datamodel'
import { SchemaWithMetadata } from '../types'
import {
  INPUT_DATABASE_CREDENTIALS_ELEMENTS,
  SELECT_DATABASE_SCHEMAS_ELEMENTS,
  SELECT_DATABASE_TYPE_ELEMENTS,
  SELECT_LANGUAGE_ELEMENTS,
  SELECT_TEMPLATE_ELEMENTS,
  SELECT_TOOL_ELEMENTS,
} from './prompts-elements'

export enum Step {
  SELECT_DATABASE_TYPE = 1,
  INPUT_DATABASE_CREDENTIALS = 2,
  SELECT_DATABASE_SCHEMA = 3,
  SELECT_TOOL = 4,
  SELECT_LANGUAGE = 5,
  SELECT_TEMPLATE = 6,
}

export const formByStep = {
  [Step.SELECT_DATABASE_TYPE]: () => SELECT_DATABASE_TYPE_ELEMENTS,
  [Step.INPUT_DATABASE_CREDENTIALS]: (dbType: DatabaseType) => INPUT_DATABASE_CREDENTIALS_ELEMENTS(dbType),
  [Step.SELECT_DATABASE_SCHEMA]: (schemas: SchemaWithMetadata[]) => SELECT_DATABASE_SCHEMAS_ELEMENTS(schemas),
  [Step.SELECT_TOOL]: () => SELECT_TOOL_ELEMENTS,
  [Step.SELECT_LANGUAGE]: () => SELECT_LANGUAGE_ELEMENTS,
  [Step.SELECT_TEMPLATE]: () => SELECT_TEMPLATE_ELEMENTS,
}

// The `Step.SELECT_DATABASE_TYPE` is not used but hardcoded for convenience
export const stepsByDatabaseType: Record<DatabaseType, Step[]> = {
  sqlite: [Step.SELECT_DATABASE_TYPE, Step.SELECT_TOOL, Step.SELECT_LANGUAGE, Step.SELECT_TEMPLATE],
  mysql: [Step.SELECT_DATABASE_TYPE, Step.INPUT_DATABASE_CREDENTIALS, Step.SELECT_DATABASE_SCHEMA],
  postgres: [Step.SELECT_DATABASE_TYPE, Step.INPUT_DATABASE_CREDENTIALS, Step.SELECT_DATABASE_SCHEMA],
  mongo: [],
}
