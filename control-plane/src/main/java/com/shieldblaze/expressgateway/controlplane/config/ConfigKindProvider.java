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

/**
 * SPI interface for registering custom {@link ConfigKind} types.
 *
 * <p>Implementations are discovered via {@link java.util.ServiceLoader} and
 * automatically registered with the {@link ConfigKindRegistry} at class load time.
 * Third-party extensions can define their own config kinds by implementing this
 * interface and providing a {@code META-INF/services} entry.</p>
 */
public interface ConfigKindProvider {

    /**
     * Returns the registration details for the config kind provided by this implementation.
     *
     * @return The kind registration containing the kind, spec class, and validator
     */
    ConfigKindRegistration registration();
}
