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

(defvar c3edit--buffers nil
  "Alist mapping active buffers to their backend IDs.")

;; TODO Replace this with a callback-based approach with a macro in which a
;; function can be registered to handle the next message.
(defvar c3edit--currently-creating-buffer nil
  "Buffer currently in the process of being created on the backend.")

(defvar c3edit--synced-changes-p nil
  "Whether current changes being inserted are from backend.
Dynamically-scoped variable to prevent infinitely-recursing changes.")

(defsubst c3edit--send-message (message)
  "Serialize MESSAGE into JSON and send it to the c3edit backend."
  (process-send-string
   c3edit--process (concat (json-encode message) "\n")))

(defun c3edit-start ()
  "Start the c3edit backend.
Start as server if SERVER is non-nil."
  (interactive)
  (when c3edit--process
    (user-error "Backend for c3edit is already running"))
  (let ((command (list c3edit-backend-path)))
    (when c3edit-port
      (setq command (nconc command `("--port" ,(number-to-string c3edit-port)))))
    (setq c3edit--process (make-process
                           :name "c3edit"
                           :command command
                           :connection-type 'pipe
                           :filter #'c3edit--process-filter
                           :stderr (get-buffer-create "*c3edit log*"))))
  (add-hook 'after-change-functions #'c3edit--after-change-function)
  (add-hook 'post-command-hook #'c3edit--post-command-function))

(defun c3edit-stop ()
  "Kill c3edit backend."
  (interactive)
  (unless c3edit--process
    (user-error "Backend for c3edit is not running"))
  (kill-process c3edit--process)
  (setq c3edit--process nil
        c3edit--buffers nil)
  (remove-hook 'after-change-functions #'c3edit--after-change-function)
  (remove-hook 'post-command-hook #'c3edit--post-command-function))

(defun c3edit-add-peer (address)
  "Add a peer at ADDRESS."
  (interactive "sAddress: ")
  (c3edit--send-message `((type . "add_peer")
                          (address . ,address))))

(defun c3edit-create-document (buffer)
  "Create a new c3edit document with BUFFER's contents.
When called interactively, BUFFER is the current buffer."
  (interactive (list (current-buffer)))
  (c3edit--send-message `((type . "create_document")
                          (name . ,(buffer-name buffer))
                          (initial_content . ,(with-current-buffer buffer
                                                (buffer-string)))))
  (setq c3edit--currently-creating-buffer buffer))

(defun c3edit-join-document (id)
  "Join document with ID."
  (interactive "sDocument ID: ")
  (c3edit--send-message `((type . "join_document")
                          (id . ,id))))

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

(defun c3edit--handle-create-document-response (id)
  "Handle `create_document_response` message with data ID."
  (push `(,c3edit--currently-creating-buffer . ,id)
        c3edit--buffers)
  (message "Document created with ID %s" id))

(defun c3edit--handle-join-document-response (id content)
  "Handle `join_document_response` for document ID with initial CONTENT."
  (let ((buffer (get-buffer-create id)))
    (with-current-buffer buffer
      (erase-buffer)
      (insert content))
    (push `(,buffer . ,id) c3edit--buffers)
    (pop-to-buffer buffer)
    (message "Joined document with ID %s" id)))

(defun c3edit--handle-change (id change)
  "Update buffer to reflect CHANGE in document ID.
CHANGE should be a variant of the `Change' enum, deserialized into an
alist."
  (let-alist change
    (with-current-buffer (car (rassoc id c3edit--buffers))
      (save-excursion
        (pcase .type
          ("insert"
           (goto-char (1+ .index))
           (insert .text))
          ("delete"
           (delete-region
            (1+ .index)
            (+ 1 .index .len))))))))

(defun c3edit--handle-new-cursor-location (id position)
  "Update cursor position in buffer for document ID to POSITION."
  (with-current-buffer (car (rassoc id c3edit--buffers))
    (goto-char (1+ position))))

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
          ("change"
           (c3edit--handle-change .document_id .change))
          ("add_peer_response"
           (message "Successfully added peer at %s" .address))
          ("create_document_response"
           (c3edit--handle-create-document-response .id))
          ("join_document_response"
           (c3edit--handle-join-document-response .id .current_content))
          ("new_cursor_location"
           (c3edit--handle-new-cursor-location .document_id .location))
          (_
           (display-warning
            'c3edit (format "Unknown message type: %s" .type) :warning)))))))

(defun c3edit--after-change-function (beg end len)
  "Update c3edit backend after a change to buffer.
BEG, END, and LEN are as documented in `after-change-functions'."
  (when-let (((not c3edit--synced-changes-p))
             (document-id (cdr (assoc (current-buffer) c3edit--buffers))))
    (let (change)
      (if (= beg end)
          (setq change `((type . "delete")
                         (index . ,(1- beg))
                         (len . ,len)))
        (setq change `((type . "insert")
                       (index . ,(1- beg))
                       (text . ,(buffer-substring-no-properties beg end)))))
      (c3edit--send-message `((type . "change")
                              (document_id . ,document-id)
                              (change . ,change))))))

;; TODO Only update cursor position if it has changed.
;; TODO Use local hooks instead of checking current buffer.
(defun c3edit--post-command-function ()
  "Update c3edit backend with cursor position after command execution."
  (when-let ((c3edit--process)
             (document-id (cdr (assoc (current-buffer) c3edit--buffers))))
    (c3edit--send-message `((type . "set_cursor")
                            (document_id . ,document-id)
                            (location . ,(1- (point)))))))

(provide 'c3edit)

;;; c3edit.el ends here
