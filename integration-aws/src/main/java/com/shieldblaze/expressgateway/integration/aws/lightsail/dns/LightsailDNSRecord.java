/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

import com.shieldblaze.expressgateway.integration.dns.DNSRecord;
import software.amazon.awssdk.services.lightsail.model.DomainEntry;

import java.net.InetAddress;
import java.net.UnknownHostException;
import java.util.Objects;

public final class LightsailDNSRecord implements DNSRecord {

    private final String fqdn;
    private final InetAddress target;
    private final RecordType recordType;

    public static LightsailDNSRecord of(String fqdn, InetAddress target, RecordType recordType) {
        return new LightsailDNSRecord(fqdn, target, recordType);
    }

    private LightsailDNSRecord(String fqdn, InetAddress target, RecordType recordType) {
        this.fqdn = Objects.requireNonNull(fqdn, "FQDN");
        this.target = Objects.requireNonNull(target, "Target");
        this.recordType = Objects.requireNonNull(recordType, "RecordType");
    }

    @Override
    public String fqdn() {
        return fqdn;
    }

    @Override
    public InetAddress target() {
        return target;
    }

    @Override
    public RecordType type() {
        return recordType;
    }

    @Override
    public long ttl() {
        return -1L;
    }

    @Override
    public String providerName() {
        return "AWS-LIGHTSAIL-DNS";
    }

    @Override
    public String toString() {
        return "LightsailDNSRecord{" +
                "fqdn='" + fqdn + '\'' +
                ", target=" + target +
                ", recordType=" + recordType +
                '}';
    }

    public static LightsailDNSRecord buildFrom(DomainEntry domainEntry) {
        RecordType recordType;
        if (domainEntry.type().equalsIgnoreCase("A")) {
            recordType = RecordType.A;
        } else if (domainEntry.type().equalsIgnoreCase("AAAA")) {
            recordType = RecordType.AAAA;
        } else {
            throw new IllegalArgumentException("Unsupported Record Type: " + domainEntry.type());
        }

        try {
            return of(domainEntry.name(), InetAddress.getByName(domainEntry.target()), recordType);
        } catch (UnknownHostException e) {
            // This can never happen
            return null;
        }
    }
}
