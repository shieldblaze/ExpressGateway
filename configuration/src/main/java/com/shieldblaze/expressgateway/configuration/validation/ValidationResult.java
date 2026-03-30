/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.configuration.validation;

import java.util.Collections;
import java.util.List;
import java.util.Objects;

/**
 * Result of a configuration validation operation.
 *
 * @param valid  Whether the configuration is valid (no ERROR-level errors)
 * @param errors The list of validation errors; empty if valid
 */
public record ValidationResult(boolean valid, List<ValidationError> errors) {

    private static final ValidationResult SUCCESS = new ValidationResult(true, List.of());

    public ValidationResult {
        Objects.requireNonNull(errors, "errors");
        errors = List.copyOf(errors);
    }

    /**
     * Returns a successful validation result with no errors.
     */
    public static ValidationResult success() {
        return SUCCESS;
    }

    /**
     * Create a validation result from a list of errors.
     * The result is valid only if there are no ERROR-severity entries.
     *
     * @param errors The validation errors
     * @return The validation result
     */
    public static ValidationResult of(List<ValidationError> errors) {
        Objects.requireNonNull(errors, "errors");
        if (errors.isEmpty()) {
            return SUCCESS;
        }
        boolean hasErrors = errors.stream()
                .anyMatch(e -> e.severity() == ValidationError.Severity.ERROR);
        return new ValidationResult(!hasErrors, errors);
    }

    /**
     * Returns the count of errors at each severity level.
     */
    public String summary() {
        if (errors.isEmpty()) {
            return "valid";
        }
        long errorCount = errors.stream().filter(e -> e.severity() == ValidationError.Severity.ERROR).count();
        long warnCount = errors.stream().filter(e -> e.severity() == ValidationError.Severity.WARNING).count();
        long infoCount = errors.stream().filter(e -> e.severity() == ValidationError.Severity.INFO).count();
        return "errors=" + errorCount + ", warnings=" + warnCount + ", info=" + infoCount;
    }

    /**
     * A single validation error.
     *
     * @param field    The field path that failed validation (e.g. "port", "nodes[0]")
     * @param message  The human-readable error message
     * @param severity The error severity
     */
    public record ValidationError(String field, String message, Severity severity) {

        public ValidationError {
            Objects.requireNonNull(field, "field");
            Objects.requireNonNull(message, "message");
            Objects.requireNonNull(severity, "severity");
        }

        /**
         * Convenience factory for ERROR-level validation errors.
         */
        public static ValidationError error(String field, String message) {
            return new ValidationError(field, message, Severity.ERROR);
        }

        /**
         * Convenience factory for WARNING-level validation errors.
         */
        public static ValidationError warning(String field, String message) {
            return new ValidationError(field, message, Severity.WARNING);
        }

        /**
         * Convenience factory for INFO-level validation notices.
         */
        public static ValidationError info(String field, String message) {
            return new ValidationError(field, message, Severity.INFO);
        }

        public enum Severity {
            ERROR,
            WARNING,
            INFO
        }
    }
}
