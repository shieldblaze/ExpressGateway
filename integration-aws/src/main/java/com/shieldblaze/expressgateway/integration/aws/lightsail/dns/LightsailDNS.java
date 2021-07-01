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

import com.shieldblaze.expressgateway.common.annotation.Async;
import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.integration.dns.DNSAddRecord;
import com.shieldblaze.expressgateway.integration.dns.DNSRemoveRecord;
import com.shieldblaze.expressgateway.integration.aws.lightsail.events.LightsailDNSAddedEvent;
import com.shieldblaze.expressgateway.integration.aws.lightsail.events.LightsailDNSRemovedEvent;
import software.amazon.awssdk.services.lightsail.LightsailClient;
import software.amazon.awssdk.services.lightsail.model.CreateDomainEntryRequest;
import software.amazon.awssdk.services.lightsail.model.CreateDomainEntryResponse;
import software.amazon.awssdk.services.lightsail.model.DeleteDomainEntryRequest;
import software.amazon.awssdk.services.lightsail.model.DeleteDomainEntryResponse;
import software.amazon.awssdk.services.lightsail.model.DomainEntry;
import software.amazon.awssdk.services.lightsail.model.OperationStatus;

public final class LightsailDNS implements DNSAddRecord<LightsailDNSRecordBody, LightsailDNSAddedEvent>,
        DNSRemoveRecord<LightsailDNSRecordBody, LightsailDNSRemovedEvent> {

    private final LightsailClient lightsailClient;

    LightsailDNS(LightsailClient lightsailClient) {
        this.lightsailClient = lightsailClient;
    }

    @Async
    @Override
    public LightsailDNSAddedEvent addRecord(LightsailDNSRecordBody lightsailDNSRecordBody) {
        LightsailDNSAddedEvent event = new LightsailDNSAddedEvent();

        GlobalExecutors.submitTask(() -> {
           try {
               DomainEntry domainEntry = DomainEntry.builder()
                       .name(lightsailDNSRecordBody.name())
                       .target(lightsailDNSRecordBody.target())
                       .type(lightsailDNSRecordBody.type())
                       .build();

               CreateDomainEntryRequest createDomainEntryRequest = CreateDomainEntryRequest.builder()
                       .domainName(lightsailDNSRecordBody.domainName())
                       .domainEntry(domainEntry)
                       .build();

               CreateDomainEntryResponse createDomainEntryResponse = lightsailClient.createDomainEntry(createDomainEntryRequest);

               OperationStatus operationStatus = createDomainEntryResponse.operation().status();
               if (operationStatus == OperationStatus.COMPLETED) {
                   event.trySuccess(true);
               } else {
                   event.tryFailure(new IllegalArgumentException("Operation Status expected to be COMPLETED but got: " + operationStatus));
               }
           } catch (Exception ex) {
               event.tryFailure(ex);
           }
        });

        return event;
    }

    @Async
    @Override
    public LightsailDNSRemovedEvent removeRecord(LightsailDNSRecordBody lightsailDNSRecordBody) {
        LightsailDNSRemovedEvent event = new LightsailDNSRemovedEvent();

        GlobalExecutors.submitTask(() -> {
           try {
               DomainEntry domainEntry = DomainEntry.builder()
                       .name(lightsailDNSRecordBody.name())
                       .target(lightsailDNSRecordBody.target())
                       .type(lightsailDNSRecordBody.type())
                       .build();

               DeleteDomainEntryResponse deleteDomainEntryResponse = lightsailClient.deleteDomainEntry(DeleteDomainEntryRequest.builder()
                       .domainEntry(domainEntry)
                       .build());

               OperationStatus operationStatus = deleteDomainEntryResponse.operation().status();
               if (operationStatus == OperationStatus.COMPLETED) {
                   event.trySuccess(true);
               } else {
                   event.tryFailure(new IllegalArgumentException("Operation Status expected to be COMPLETED but got: " + operationStatus));
               }
           } catch (Exception ex) {
               event.tryFailure(ex);
           }
        });

        return event;
    }
}
