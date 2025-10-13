Validation Examples
==================

This document shows Sphinx Ultra's new constraint validation system with simple examples.

Working Requirements (These Pass Validation)
--------------------------------------------

.. req:: REQ-001
   :title: User Login System
   :status: complete
   :priority: high
   
   Users can log in with username and password.

.. req:: REQ-002  
   :title: Password Reset
   :status: verified
   :priority: medium
   
   Users can reset their password via email.

Failing Requirements (These Will Fail Validation)
-------------------------------------------------

.. req:: REQ-003
   :title: Security Audit
   :status: open
   :priority: critical
   
   ❌ This will fail - Critical priority items must be complete!

.. req:: REQ-004
   :title: Data Backup
   :status: pending
   :priority: high
   
   ❌ This will fail - High priority items must be complete!

.. req:: REQ-005
   :title: Invalid Priority Example
   :status: complete
   :priority: super-high
   
   ❌ This will fail - Invalid priority value!

Running Validation
------------------

To see these validation failures in action:

.. code-block:: bash

   cargo run --example constraint_validation

You'll see which requirements pass and which fail the validation rules!