#pragma once

// Process teardown needs only the submission admission/lifetime boundary,
// not Android NDK payload types or compositor implementation headers.
extern "C" void CloseSurfaceTransactionSubmissionAdmission();
extern "C" bool PollSurfaceTransactionSubmissionQuiesced();
extern "C" bool ResetSurfaceTransactionSubmission();
