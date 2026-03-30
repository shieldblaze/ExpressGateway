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

import java.util.Objects;

/**
 * Associates a {@link ConfigKind} with its spec class and validator.
 *
 * <p>This record is produced by {@link ConfigKindProvider} implementations
 * and consumed by the {@link ConfigKindRegistry} to enable type-safe
 * deserialization and validation of config payloads.</p>
 *
 * @param kind      The config kind being registered
 * @param specClass The concrete {@link ConfigSpec} implementation class for this kind
 * @param validator The validator for specs of this kind
 */
public record ConfigKindRegistration(
        ConfigKind kind,
        Class<? extends ConfigSpec> specClass,
        ConfigSpecValidator validator
) {
    public ConfigKindRegistration {
        Objects.requireNonNull(kind, "kind");
        Objects.requireNonNull(specClass, "specClass");
        Objects.requireNonNull(validator, "validator");
    }
}
