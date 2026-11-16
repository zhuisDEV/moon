-- Add diagnostics without rebuilding jobs or rewriting accepted review history.
-- The original last_error remains the legacy category. Status readers expose
-- COALESCE(failure_code,last_error); runtime transitions clear obsolete details.
ALTER TABLE source_learning_jobs ADD COLUMN failure_code TEXT CHECK (
    failure_code IS NULL OR (failure_code IN (
        'model_failed','invalid_response','interrupted','runtime_unavailable',
        'runtime_setup_failed','authentication_failed','model_unavailable',
        'model_route_mismatch','model_timeout'
    ) AND status IN ('queued','dead')
    AND last_error IS CASE failure_code
        WHEN 'invalid_response' THEN 'invalid_response'
        WHEN 'interrupted' THEN 'interrupted' ELSE 'model_failed' END
    AND (failure_code IN ('model_failed','invalid_response','interrupted','model_timeout')
         OR status='dead'))
);
ALTER TABLE topic_jobs ADD COLUMN failure_code TEXT CHECK (
    failure_code IS NULL OR (failure_code IN (
        'model_failed','invalid_response','interrupted','runtime_unavailable',
        'runtime_setup_failed','authentication_failed','model_unavailable',
        'model_route_mismatch','model_timeout'
    ) AND status IN ('queued','dead')
    AND last_error IS CASE failure_code
        WHEN 'invalid_response' THEN 'invalid_response'
        WHEN 'interrupted' THEN 'interrupted' ELSE 'model_failed' END
    AND (failure_code IN ('model_failed','invalid_response','interrupted','model_timeout')
         OR status='dead'))
);
