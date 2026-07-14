# Tally Development Details

## Log Structure
The structure of the log that needs to be present in SQLite for tally deamon to read it and generate hashes.

### SESSION_START

|  Modified Structure   | Tally Spec Structure |
|-----|-------|
|     | record_type |
|     | schema_version |
|     | session_id |
|     | agent_id |
|     | agent_version |
|     | principal |
|     | &nbsp;&nbsp;type |
|     | &nbsp;&nbsp;id |
|     | authority_scope_hash |
|     | authority_scope_uri |
|     | authority_granted_at |
|     | authority_expires_at |
|     | delegation_chain |
|     | &nbsp;&nbsp;depth |
|     | &nbsp;&nbsp;chain_hash |
|     | &nbsp;&nbsp;chain |
|     | &nbsp;&nbsp;chain_uri |
|     | session_started_at |
|     | anchor_receipt |

### INSTRUCTION_RECEIVED

|  Modified Structure   | Tally Spec Structure |
|-----|-------|
|     | record_type |
|     | schema_version |
|     | session_id |
|     | instruction_id |
|     | sender |
|     | &nbsp;&nbsp;id |
|     | &nbsp;&nbsp;signature |
|     | instruction_hash |
|     | instruction_uri |
|     | instruction_received_at |
|     | context_snapshot_hash |
|     | context_snapshot_uri |
|     | declared_intent |
|     | &nbsp;&nbsp;summary |
|     | &nbsp;&nbsp;detail_hash |
|     | &nbsp;&nbsp;detail_uri |
|     | anchor_receipt |

### ACTION_TAKEN

|  Modified Structure   | Tally Spec Structure |
|-----|-------|
|     | record_type |
|     | schema_version |
|     | session_id |
|     | action_id |
|     | instruction_id |
|     | action_type |
|     | tool |
|     | &nbsp;&nbsp;server |
|     | &nbsp;&nbsp;name |
|     | &nbsp;&nbsp;params_hash |
|     | &nbsp;&nbsp;params_uri |
|     | pre_state_hash |
|     | pre_state_uri |
|     | post_state_hash |
|     | post_state_uri |
|     | action_timestamp |
|     | anchor_receipt |
|     | deviance_flag |
|     | &nbsp;&nbsp;deviated |
|     | &nbsp;&nbsp;delta_category |
|     | &nbsp;&nbsp;delta_hash |
|     | &nbsp;&nbsp;delta_uri |

### RESULT_RECEIVED

|  Modified Structure   | Tally Spec Structure |
|-----|-------|
|     | record_type |
|     | schema_version |
|     | session_id |
|     | action_id |
|     | result_hash |
|     | result_uri |
|     | result_received_at |
|     | result_interpretation |
|     | &nbsp;&nbsp;summary |
|     | &nbsp;&nbsp;detail_hash |
|     | &nbsp;&nbsp;detail_uri |
|     | exception |
|     | &nbsp;&nbsp;occurred |
|     | &nbsp;&nbsp;type |
|     | &nbsp;&nbsp;description_hash |
|     | &nbsp;&nbsp;description_uri |

### HANDOFF

|  Modified Structure   | Tally Spec Structure |
|-----|-------|
|     | record_type |
|     | schema_version |
|     | session_id |
|     | handoff_id |
|     | emitting_party |
|     | sender |
|     | &nbsp;&nbsp;agent_id |
|     | &nbsp;&nbsp;org_id |
|     | &nbsp;&nbsp;signature |
|     | receiver |
|     | &nbsp;&nbsp;agent_id |
|     | &nbsp;&nbsp;org_id |
|     | &nbsp;&nbsp;signature |
|     | &nbsp;&nbsp;acknowledged_at |
|     | payload_hash |
|     | payload_uri |
|     | handoff_timestamp |
|     | acknowledgement_status |
|     | anchor_receipt |

### SESSION_END

|  Modified Structure   | Tally Spec Structure |
|-----|-------|
|     | record_type |
|     | schema_version |
|     | session_id |
|     | outcome |
|     | outcome_hash |
|     | outcome_uri |
|     | human_review |
|     | &nbsp;&nbsp;required |
|     | &nbsp;&nbsp;reviewer_id |
|     | &nbsp;&nbsp;approved_at |
|     | &nbsp;&nbsp;approval_hash |
|     | session_ended_at |
|     | anchor_receipt |

### HEARTBEAT

|  Modified Structure   | Tally Spec Structure |
|-----|-------|
|     | record_type |
|     | schema_version |
|     | agent_id |
|     | anchor_instance_id |
|     | active_sessions |
|     | timestamp |
|     | anchor_receipt |


```
{
    "record_type":"",
    "schema_version": "",
    "session_id":"",
    "agent_id":"",
    "agent_type":"",
    "agent_version":"",
    "principal":{"type":"","id":""},
    "source":"",
    "session_started_at":""
}
```
When recored_type is Heartbeat
```
{
    "record_type":"HEARTBEAT",
    "session_id":"",
    "timestamp":""
}
```