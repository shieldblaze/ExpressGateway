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
package com.shieldblaze.expressgateway.config;

import java.util.Collections;
import java.util.List;
import java.util.Objects;

/**
 * Thrown when gateway configuration validation fails. Collects ALL violations
 * rather than failing on the first one, so operators can fix every problem
 * in a single iteration.
 */
public class ConfigValidationException extends RuntimeException {

    private final List<String> violations;

    /**
     * Creates a new validation exception with the collected violations.
     *
     * @param violations non-empty list of human-readable violation descriptions
     * @throws IllegalArgumentException if violations is null or empty
     */
    public ConfigValidationException(List<String> violations) {
        Objects.requireNonNull(violations, "violations list must not be null");
        if (violations.isEmpty()) {
            throw new IllegalArgumentException("violations list must not be empty");
        }
        super(formatMessage(violations));
        this.violations = Collections.unmodifiableList(violations);
    }

    /**
     * Returns the unmodifiable list of all validation violations.
     */
    public List<String> violations() {
        return violations;
    }

    private static String formatMessage(List<String> violations) {
        if (violations == null || violations.isEmpty()) {
            return "Configuration validation failed";
        }
        StringBuilder sb = new StringBuilder("Configuration validation failed with ")
                .append(violations.size())
                .append(" violation(s):\n");
        for (int i = 0; i < violations.size(); i++) {
            sb.append("  ").append(i + 1).append(". ").append(violations.get(i));
            if (i < violations.size() - 1) {
                sb.append('\n');
            }
        }
        return sb.toString();
    }
}
