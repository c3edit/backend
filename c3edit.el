;;; c3edit.el --- Real-time cross-editor collaborative editing -*- lexical-binding: t -*-

;; Author: Adam Zhang <898544@lcps.org>
;; Version: 0.0.1
;; Package-Requires: ((emacs "25.1"))
;; Homepage: https://github.com/adam-zhang-lcps/c3edit

;; This file is not part of GNU Emacs

;; This program is free software: you can redistribute it and/or modify
;; it under the terms of the GNU General Public License as published by
;; the Free Software Foundation, either version 3 of the License, or
;; (at your option) any later version.

;; This program is distributed in the hope that it will be useful,
;; but WITHOUT ANY WARRANTY; without even the implied warranty of
;; MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
;; GNU General Public License for more details.

;; You should have received a copy of the GNU General Public License
;; along with this program.  If not, see <https://www.gnu.org/licenses/>.


;;; Commentary:

;; WIP

;;; Code:

(require 'json)
(require 'map)

(defgroup c3edit nil
  "Real-time cross-editor collaborative editing."
  :group 'editing)

(defcustom c3edit-backend-path (executable-find "c3edit")
  "Path to c3edit backend binary."
  :type '(file :must-match t))

(defcustom c3edit-port nil
  "Port to listen for incoming connections on."
  :type 'natnum)

(defvar c3edit--process nil
  "Process for c3edit backend.")

(defvar c3edit--buffer nil
  "Current active c3edit buffer.")

(defvar c3edit--synced-changes-p nil
  "Whether current changes being inserted are from backend.
Dynamically-scoped variable to prevent infinitely-recursing changes.")

(defun c3edit-start ()
  "Start the c3edit backend.
Start as server if SERVER is non-nil."
  (interactive)
  (when c3edit--process
    (user-error "Backend for c3edit is already running"))
  (let ((command (list c3edit-backend-path)))
    (when c3edit-port
      (setq command (nconc command `("--port" ,c3edit-port))))
    (setq c3edit--process (make-process
                           :name "c3edit"
                           :command command
                           :filter #'c3edit--process-filter
                           :stderr (get-buffer-create "*c3edit log*"))))
  (setq c3edit--buffer (current-buffer))
  (add-hook 'after-change-functions #'c3edit--after-change-function))

(defun c3edit-stop ()
  "Kill c3edit backend."
  (interactive)
  (unless c3edit--process
    (user-error "Backend for c3edit is not running"))
  (kill-process c3edit--process)
  (setq c3edit--process nil)
  (remove-hook 'after-change-functions #'c3edit--after-change-function))

(defun c3edit-add-peer (address)
  "Add a peer at ADDRESS."
  (interactive "sAddress: ")
  (c3edit--send-message `((type . "add_peer")
                          (address . ,address))))

(defsubst c3edit--send-message (message)
  "Serialize MESSAGE into JSON and send it to the c3edit backend."
  (process-send-string
   c3edit--process (concat (json-encode message) "\n")))

(defun c3edit--send-initial-data ()
  "Send initial buffer contents to backend process."
  (c3edit--send-message `((type . "create_document")
                          (initial_content . ,(buffer-string)))))

(defun c3edit--json-read-all (string)
  "Read all JSON objects from STRING.
Returns list of read objects."
  (let (data)
    (with-temp-buffer
      (insert string)
      (goto-char (point-min))
      (condition-case _err
          (while t
            (push (json-read) data))
        (json-end-of-file)))
    (nreverse data)))

(defun c3edit--handle-change (change)
  "Update buffer to reflect CHANGE.
CHANGE should be a variant of the `Change' enum, deserialized into an
alist."
  (with-current-buffer c3edit--buffer
    (save-excursion
      (let-alist change
        (pcase .type
          ("insert"
           (goto-char (1+ .index))
           (insert .text))
          ("delete"
           (delete-region
            (1+ .index)
            (+ 1 .index .len))))))))

(defun c3edit--process-filter (_process text)
  "Process filter for c3edit backend messages.
Processes message from TEXT."
  (message "Received data: %s" text)
  ;; Emacs process handling may return many lines at once, we have to make sure
  ;; to read them all in order.
  (let* ((data (c3edit--json-read-all text))
         (c3edit--synced-changes-p t))
    (dolist (message data)
      (let-alist message
        (pcase .type
          ("change" (c3edit--handle-change .change))
          ("peer_added_response" (message "Successfully added peer at %s" .address)))))))

(defun c3edit--after-change-function (beg end len)
  "Update c3edit backend after a change to buffer.
BEG, END, and LEN are as documented in `after-change-functions'."
  (when (and (not c3edit--synced-changes-p)
             (equal (current-buffer) c3edit--buffer))
    (let (change)
      (if (= beg end)
          (setq change `((type . "delete")
                         (index . ,(1- beg))
                         (len . ,len)))
        (setq change `((type . "insert")
                       (index . ,(1- beg))
                       (text . ,(buffer-substring-no-properties beg end)))))
      (c3edit--send-message `((type . "change")
                              (change . ,change))))))

(provide 'c3edit)

;;; c3edit.el ends here
