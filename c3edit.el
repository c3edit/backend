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

(defcustom c3edit-backend-path (executable-find "c3edit")
  "Path to c3edit backend binary.")

(defvar c3edit--process nil
  "Process for c3edit backend.")

(defvar c3edit--synced-changes-p nil
  "Whether current changes being inserted are from backend.
Dynamically-scoped variable to prevent infinitely-recursing changes.")

(defun c3edit-start (&optional server)
  "Start the c3edit backend.
Start as server if SERVER is non-nil."
  (interactive (list (y-or-n-p "Start as server?")))
  (let ((address)
        (command (list c3edit-backend-path)))
    (if server
        (setq command (append command (list "server")))
      (setq address (read-string "Address: "))
      (setq command (append command (list address))))
    (setq c3edit--process (make-process
                           :name "c3edit"
                           :command command
                           :filter #'c3edit--process-filter
                           :stderr (get-buffer-create "*c3edit log*")))))

(defun c3edit-stop ()
  "Kill c3edit backend."
  (interactive)
  (kill-process c3edit--process))

(defun c3edit--process-filter (_process text)
  "Process filter for c3edit backend messages.
Processes message from TEXT."
  (message "Received data: %s" text)
  (let* ((data (json-read-from-string text))
         (c3edit--synced-changes-p t))
    (with-current-buffer "c3edit"
      (save-excursion
        (pcase (caar data)
          ('insert
           (goto-char (1+ (map-nested-elt data '(insert index))))
           (insert (map-nested-elt data '(insert text))))
          ('delete
           (delete-region
            (1+ (map-nested-elt data '(delete index)))
            (+ (map-nested-elt data '(delete index))
               (map-nested-elt data '(delete len))
               1))))))))

(defun c3edit--after-change-function (beg end len)
  "Update c3edit backend after a change to buffer.
BEG, END, and LEN are as documented in `after-change-functions'."
  (when-let (((not c3edit--synced-changes-p))
             ((string= (buffer-name (current-buffer))
                       "c3edit"))
             (data ""))
    (if (= beg end)
        (setq data `((delete . ((index . ,(1- beg))
                                (len . ,len)))))
      (setq data `((insert . ((index . ,(1- beg))
                              (text . ,(buffer-substring-no-properties beg end)))))))
    (process-send-string c3edit--process
                         (format "%s\n" (json-encode data)))))

(add-hook 'after-change-functions #'c3edit--after-change-function)

(provide 'c3edit)

;;; c3edit.el ends here
