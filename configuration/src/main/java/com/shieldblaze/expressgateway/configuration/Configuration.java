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
package com.shieldblaze.expressgateway.configuration;

/**
 * Interface for Configuration
 */
public interface Configuration<T> {

    /**
     * Check if this Configuration is validated or not
     */
    boolean validated();

    /**
     * Validate this Configuration
     *
     * @return This configuration instance
     * @throws IllegalArgumentException If there is an error during validation
     */
    T validate();

    /**
     * Friendly name of this Configuration.
     * It is made up of {id:profileName}
     */
    default String friendlyName() {
        return getClass().getSimpleName();
    }

    /**
     * Assert successful validation check of this Configuration.
     * This method must be called whenever this Configuration
     * values are being accessed.
     */
    default void assertValidated() {
        assert validated() : "Configuration must be validated";
    }
}
