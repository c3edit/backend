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

(setq my/test (make-process
               :name "c3edit"
               :command '("~/git/c3edit/target/debug/c3edit")
               :filter #'my/c3-filter
               :stderr (get-buffer-create "*test*")))

(kill-process my/test)

(process-send-string my/test "hello world\n")
(process-send-eof my/test)

(defvar my/synced-changes nil)

(defun my/c3-filter (process text)
  "Coding go brrrrr."
  (message "Received data: %s" text)
  (let* ((data (json-read-from-string text))
         (my/synced-changes t))
    (with-current-buffer "test"
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

(defun my/change-test (beg end len)
  "Test time!"
  (when-let (((not my/synced-changes))
             ((string= (buffer-name (current-buffer))
                       "test"))
             (data ""))
    (if (= beg end)
        (setq data `((delete . ((index . ,(1- beg))
                                (len . ,len)))))
      (setq data `((insert . ((index . ,(1- beg))
                              (text . ,(buffer-substring-no-properties beg end)))))))
    (process-send-string my/test
                         (format "%s\n" (json-encode data)))))

(add-hook 'after-change-functions #'my/change-test)

(provide 'c3edit)

;;; c3edit.el ends here
