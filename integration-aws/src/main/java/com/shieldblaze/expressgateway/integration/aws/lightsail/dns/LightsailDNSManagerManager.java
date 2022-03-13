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

import com.shieldblaze.expressgateway.integration.dns.DNSManager;
import com.shieldblaze.expressgateway.integration.dns.DNSRecord;
import com.shieldblaze.expressgateway.integration.aws.AWS;
import com.shieldblaze.expressgateway.integration.event.DNSAddedEvent;
import com.shieldblaze.expressgateway.integration.event.DNSRemovedEvent;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.auth.credentials.AwsCredentialsProvider;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.lightsail.LightsailClient;
import software.amazon.awssdk.services.lightsail.model.Domain;
import software.amazon.awssdk.services.lightsail.model.GetDomainsRequest;
import software.amazon.awssdk.services.lightsail.model.GetDomainsResponse;

import java.io.Closeable;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

public final class LightsailDNSManagerManager extends AWS implements DNSManager<LightsailDNSRecordBody>, Runnable, Closeable {

    private static final Logger logger = LogManager.getLogger(LightsailDNSManagerManager.class);

    private final ScheduledExecutorService executorService = Executors.newSingleThreadScheduledExecutor();
    private final ScheduledFuture<?> scheduledFuture;

    private final List<DNSRecord> dnsRecordList = new CopyOnWriteArrayList<>();
    private final LightsailClient lightsailClient;
    private final LightsailDNS lightsailDns;
    private final String domainName;

    public LightsailDNSManagerManager(AwsCredentialsProvider awsCredentialsProvider, String domainName) {
        super(awsCredentialsProvider, Region.US_EAST_1);
        this.domainName = Objects.requireNonNull(domainName, "DomainName");

        lightsailClient = LightsailClient.builder()
                .credentialsProvider(awsCredentialsProvider)
                .region(Region.US_EAST_1)
                .build();

        lightsailDns = new LightsailDNS(lightsailClient);
        scheduledFuture = executorService.scheduleAtFixedRate(this, 0, 30, TimeUnit.SECONDS);
    }

    @Override
    public void run() {
        List<DNSRecord> dnsList = new ArrayList<>();
        searchRecords(dnsList, null);

        dnsRecordList.clear();
        dnsRecordList.addAll(dnsList);
    }

    private void searchRecords(List<DNSRecord> dnsList, String pageToken) {
        GetDomainsResponse getDomainsResponse;
        if (pageToken == null) {
            getDomainsResponse = lightsailClient.getDomains();
        } else {
            getDomainsResponse = lightsailClient.getDomains(GetDomainsRequest.builder().pageToken(pageToken).build());
        }

        Optional<Domain> optionalDomain = getDomainsResponse.domains()
                .stream()
                .filter(domain -> domain.name().equalsIgnoreCase(domainName))
                .findFirst();

        // If OptionalDomain is empty then we didn't find DomainName in this response.
        if (optionalDomain.isEmpty()) {

            /*
             * If PageToken is null then we've reached the end of results. We'll throw an exception now.
             * If PageToken is available then we'll search again recursively.
             */
            if (getDomainsResponse.nextPageToken() == null) {
                IllegalArgumentException exception = new IllegalArgumentException("Domain Name not found");
                logger.fatal(exception);
                throw exception;
            } else {
                searchRecords(dnsList, getDomainsResponse.nextPageToken());
            }
        } else {
            Domain domain = optionalDomain.get();

            domain.domainEntries()
                    .stream()
                    .filter(domainEntry -> (domainEntry.type().equalsIgnoreCase("A") || domainEntry.type().equalsIgnoreCase("AAAA")))
                    .forEach(domainEntry -> dnsList.add(LightsailDNSRecord.buildFrom(domainEntry)));
        }
    }

    @Override
    public List<DNSRecord> dnsRecords() {
        return dnsRecordList;
    }

    @Override
    public DNSAddedEvent<Boolean> add(LightsailDNSRecordBody lightsailDNSRecordBody) {
        return lightsailDns.addRecord(lightsailDNSRecordBody);
    }

    @Override
    public DNSRemovedEvent<Boolean> remove(LightsailDNSRecordBody lightsailDNSRecordBody) {
        return lightsailDns.removeRecord(lightsailDNSRecordBody);
    }

    @Override
    public void close() {
        scheduledFuture.cancel(true);
        executorService.shutdown();
        lightsailClient.close();
    }
}
