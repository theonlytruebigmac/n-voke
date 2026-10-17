export type InputType = "STRING" | "PASSWORD" | "BOOLEAN" | "INT" | "FLOAT" | "DATE_TIME";

export interface Organization {
  id: string;
  name: string;
  __typename: string;
}

export interface ScriptInputDecl {
  name: string;
  variable: string;
  type: InputType;
}

export interface Script {
  id: string;
  name: string;
  updatedAt?: string | null;
  language: string;
  description: string | null;
  timeoutSeconds: number | null;
  inputs: ScriptInputDecl[] | null;
}

export interface Asset {
  id: string;
  name: string;
  customer: { name: string } | null;
  site: { name: string } | null;
  agentConnection: { status: string } | null;
  operatingSystem?: { name: string | null } | null;
}

export type TagKind = "StaticTag" | "DynamicTag";

export interface Tag {
  id: string;
  name: string;
  __typename: TagKind;
  assetCount: number | null;
  updatedAt?: string | null;
  description?: string | null;
  organization: { id: string; name: string } | null;
  /** Dynamic tags only: the rule as a JSON string. */
  filter?: string | null;
}

/** Typed value object the API expects for script inputs and outputs. */
export interface InputValue {
  stringValue?: string | null;
  booleanValue?: boolean | null;
  intValue?: number | null;
  floatValue?: number | null;
  dateTimeValue?: string | null;
}

export interface Execution {
  id: string;
  status: string;
  exitCode: number | null;
  errorMessage: string | null;
  startedAt: string | null;
  updatedAt: string | null;
  durationMilliseconds: number | null;
  output?: {
    runAs: string | null;
    stdOutput: string | null;
    stdError: string | null;
    outputs: { name: string; variable: string; type: string; value: InputValue | null }[] | null;
  } | null;
}

export interface TaskSnapshot {
  id: string;
  /** Only present from getTask (see TASK_FIELDS in api.ts), and not on a just-created task. */
  name?: string;
  status: string;
  createdAt: string | null;
  script?: { id: string; name: string } | null;
  executions: { nodes: (Execution | null)[] } | null;
}
