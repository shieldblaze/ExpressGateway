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
package com.shieldblaze.expressgateway.integration.aws.lightsail.instance;

/**
 * Lightsail Bundles (Variants)
 */
public enum Bundle {
    MICRO_2_1("micro_2_1"),
    SMALL_2_1("small_2_1"),
    MEDIUM_2_1("medium_2_1"),
    LARGE_2_1("large_2_1"),
    XLARGE_2_1("xlarge_2_1"),
    X2LARGE_2_1("2xlarge_2_1");

    private final String bundle;

    Bundle(String bundle) {
        this.bundle = bundle;
    }

    public String bundleName() {
        return bundle;
    }
}
