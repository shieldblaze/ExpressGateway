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

import com.fasterxml.jackson.annotation.JsonCreator;
import com.fasterxml.jackson.annotation.JsonValue;
import lombok.Getter;

/**
 * Deployment environment for the gateway. Controls default behavior
 * such as log verbosity, TLS strictness, and validation sensitivity.
 */
@Getter
public enum Environment {

    DEVELOPMENT("dev"),
    STAGING("staging"),
    PRODUCTION("prod");

    @JsonValue
    private final String code;

    Environment(String code) {
        this.code = code;
    }

    /**
     * Resolves an {@link Environment} from its short code or enum name.
     *
     * @param code the short code (e.g. "dev") or enum name (e.g. "DEVELOPMENT")
     * @return the matching environment
     * @throws IllegalArgumentException if the code is not recognized
     */
    @JsonCreator
    public static Environment fromCode(String code) {
        if (code == null) {
            throw new IllegalArgumentException("Environment code must not be null");
        }
        String normalized = code.trim().toLowerCase();
        for (Environment env : values()) {
            if (env.code.equals(normalized) || env.name().equalsIgnoreCase(normalized)) {
                return env;
            }
        }
        throw new IllegalArgumentException("Unknown environment code: '" + code
                + "'. Valid values: dev, staging, prod (or DEVELOPMENT, STAGING, PRODUCTION)");
    }
}
