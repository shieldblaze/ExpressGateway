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
package com.shieldblaze.expressgateway.controlplane.config;

import java.util.List;

/**
 * Functional interface for validating a {@link ConfigSpec} and returning
 * a list of human-readable error messages.
 *
 * <p>An empty list indicates the spec is valid. This is separate from
 * {@link ConfigSpec#validate()} to allow external/pluggable validation
 * logic without modifying the spec class itself.</p>
 */
@FunctionalInterface
public interface ConfigSpecValidator {

    /**
     * Validate the given spec.
     *
     * @param spec The spec to validate
     * @return A list of validation error messages; empty if valid
     */
    List<String> validate(ConfigSpec spec);
}
