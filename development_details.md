# Tally Development Details

## SQLite Log Structure
Required SQLite log schema for tally daemon reading and hash generation.

### SESSION_START

|  SQLite Structure | Log in js-version | Tally Spec Structure | 
|-----|-------|-------|
| record_type | record_type | record_type |
| schema_version |      | schema_version |
| session_id |  session_id   | session_id |
| agnet_type |  |  |
| agent_id |  agent_id   | agent_id |
| agent_version |  agent_version | agent_version |
| principal |  principal | principal |
| &nbsp;&nbsp;type |  &nbsp;&nbsp;type | &nbsp;&nbsp;type |
| &nbsp;&nbsp;id |  &nbsp;&nbsp;id | &nbsp;&nbsp;id |
|  |     | authority_scope_hash |
|  |     | authority_scope_uri |
|  |     | authority_granted_at |
|  |     | authority_expires_at |
|  |     | delegation_chain |
|  |     | &nbsp;&nbsp;depth |
|  |     | &nbsp;&nbsp;chain_hash |
|  |     | &nbsp;&nbsp;chain |
|  |     | &nbsp;&nbsp;chain_uri |
| session_started_at |  session_started_at   | session_started_at |
|  |     | anchor_receipt |
| source | source |

### INSTRUCTION_RECEIVED

|  SQLite Structure | Log in js-version | Tally Spec Structure | 
|-----|-------|-------|
| record_type |  record_type   | record_type |
| schema_version |     | schema_version |
| session_id |  session_id   | session_id |
| instruction_id |  instruction_id   | instruction_id |
| sender |  sender   | sender |
| &nbsp;&nbsp;id |  &nbsp;&nbsp;id   | &nbsp;&nbsp;id |
|  |     | &nbsp;&nbsp;signature |
|  |     | instruction_hash |
|  |     | instruction_uri |
| instruction_received_at |  instruction_received_at   | instruction_received_at |
|  |     | context_snapshot_hash |
|  |     | context_snapshot_uri |
| declared_intent |  declared_intent   | declared_intent |
| &nbsp;&nbsp;summary |  &nbsp;&nbsp;summary   | &nbsp;&nbsp;summary |
|  |     | &nbsp;&nbsp;detail_hash |
|  |     | &nbsp;&nbsp;detail_uri |
|  |     | anchor_receipt |

### ACTION_TAKEN

|  SQLite Structure | Log in js-version | Tally Spec Structure | 
|-----|-------|-------|
| record_type |  record_type   | record_type |
| schema_version |     | schema_version |
| session_id |  session_id   | session_id |
| action_id |  action_id   | action_id |
| instruction_id |  instruction_id   | instruction_id |
| action_type |  action_type   | action_type |
| tool |  tool   | tool |
|  |     | &nbsp;&nbsp;server |
| &nbsp;&nbsp;name |  &nbsp;&nbsp;name   | &nbsp;&nbsp;name |
| &nbsp;&nbsp;params |  &nbsp;&nbsp;params  |  |
|  |     | &nbsp;&nbsp;params_hash |
|  |     | &nbsp;&nbsp;params_uri |
|  |     | pre_state_hash |
|  |     | pre_state_uri |
|  |     | post_state_hash |
|  |     | post_state_uri |
| action_timestamp |  action_timestamp   | action_timestamp |
|  |     | anchor_receipt |
| deviance_flag |  deviance_flag   | deviance_flag |
| &nbsp;&nbsp;deviated |  &nbsp;&nbsp;deviated   | &nbsp;&nbsp;deviated |
| &nbsp;&nbsp;delta_category |  &nbsp;&nbsp;delta_category   | &nbsp;&nbsp;delta_category |
|  |     | &nbsp;&nbsp;delta_hash |
|  |     | &nbsp;&nbsp;delta_uri |

### RESULT_RECEIVED

|  SQLite Structure | Log in js-version | Tally Spec Structure | 
|-----|-------|-------|
| record_type |  record_type   | record_type |
| schema_version |     | schema_version |
| session_id |  session_id   | session_id |
| action_id |  action_id   | action_id |
|  |     | result_hash |
|  |     | result_uri |
| result_received_at |  result_received_at   | result_received_at |
| result_interpretatio |  result_interpretation   | result_interpretation |
| &nbsp;&nbsp;summar |  &nbsp;&nbsp;summary   | &nbsp;&nbsp;summary |
|  |     | &nbsp;&nbsp;detail_hash |
|  |     | &nbsp;&nbsp;detail_uri |
| exception |  exception   | exception |
| &nbsp;&nbsp;occurred |  &nbsp;&nbsp;occurred   | &nbsp;&nbsp;occurred |
|  |     | &nbsp;&nbsp;type |
|  |     | &nbsp;&nbsp;description_hash |
|  |     | &nbsp;&nbsp;description_uri |

### HANDOFF

|  SQLite Structure | Log in js-version | Tally Spec Structure | 
|-----|-------|-------|
|  |     | record_type |
|  |     | schema_version |
|  |     | session_id |
|  |     | handoff_id |
|  |     | emitting_party |
|  |     | sender |
|  |     | &nbsp;&nbsp;agent_id |
|  |     | &nbsp;&nbsp;org_id |
|  |     | &nbsp;&nbsp;signature |
|  |     | receiver |
|  |     | &nbsp;&nbsp;agent_id |
|  |     | &nbsp;&nbsp;org_id |
|  |     | &nbsp;&nbsp;signature |
|  |     | &nbsp;&nbsp;acknowledged_at |
|  |     | payload_hash |
|  |     | payload_uri |
|  |     | handoff_timestamp |
|  |     | acknowledgement_status |
|  |     | anchor_receipt |

### SESSION_END

|  SQLite Structure | Log in js-version | Tally Spec Structure | 
|-----|-------|-------|
| record_type |  record_type   | record_type |
| schema_version |     | schema_version |
| session_id |  session_id   | session_id |
| outcome |  outcome   | outcome |
|  |     | outcome_hash |
|  |     | outcome_uri |
|  |     | human_review |
|  |     | &nbsp;&nbsp;required |
|  |     | &nbsp;&nbsp;reviewer_id |
|  |     | &nbsp;&nbsp;approved_at |
|  |     | &nbsp;&nbsp;approval_hash |
| session_ended_at |  session_ended_at   | session_ended_at |
|  |     | anchor_receipt |

### HEARTBEAT

|  SQLite Structure | Log in js-version | Tally Spec Structure | 
|-----|-------|-------|
| record_type |  record_type   | record_type |
| schema_version |     | schema_version |
| session_id |  session_id   | session_id |
|  |     | agent_id |
|  |     | anchor_instance_id |
|  |     | active_sessions |
| timestamp |  timestamp   | timestamp |
|  |     | anchor_receipt |

