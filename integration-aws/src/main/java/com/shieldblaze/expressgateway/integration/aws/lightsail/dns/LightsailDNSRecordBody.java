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
package com.shieldblaze.expressgateway.integration.aws.lightsail.dns;

import java.util.Objects;

public record LightsailDNSRecordBody(String name, String domainName, String type, String target) {
    public LightsailDNSRecordBody(String name, String domainName, String type, String target) {
        this.name = Objects.requireNonNull(name, "Name");
        this.domainName = Objects.requireNonNull(domainName, "DomainName");
        this.type = Objects.requireNonNull(type, "Type");
        this.target = Objects.requireNonNull(target, "Target");
    }
}
