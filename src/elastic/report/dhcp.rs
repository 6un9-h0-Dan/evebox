// Copyright (C) 2020 Jason Ish
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use super::super::eventstore::EventStore;
use crate::elastic::{self, query_string_query, request::Request};
use crate::{
    datastore::{DatastoreError, EventQueryParams},
    types::JsonValue,
};

pub async fn dhcp_report(
    ds: &EventStore,
    what: &str,
    params: &EventQueryParams,
) -> Result<JsonValue, DatastoreError> {
    let mut filters = Vec::new();

    filters.push(elastic::request::term_filter("event_type", "dhcp"));

    if let Some(dt) = params.min_timestamp {
        filters.push(elastic::request::timestamp_gte_filter(dt));
    }

    if let Some(query_string) = &params.query_string {
        filters.push(query_string_query(&query_string));
    }

    match what {
        "ack" => dhcp_report_ack(ds, filters).await,
        "request" => dhcp_report_request(ds, filters).await,
        "servers" => servers(ds, filters).await,
        "mac" => mac(ds, filters).await,
        "ip" => ip(ds, filters).await,
        _ => Err(anyhow::anyhow!("No DHCP report for {}", what).into()),
    }
}

pub async fn dhcp_report_ack(
    ds: &EventStore,
    mut filters: Vec<JsonValue>,
) -> Result<JsonValue, DatastoreError> {
    let mut request = elastic::request::new_request();
    filters.push(elastic::request::term_filter("dhcp.dhcp_type", "ack"));
    request.set_filters(filters);

    let aggs = json!({
        "client_mac": {
          "terms": {
            "field": "dhcp.client_mac.keyword",
            "size": 10000
          },
          "aggs": {
            "latest": {
              "top_hits": {
                "sort": [
                  {
                    "@timestamp": {"order": "desc"}
                  }
                ],
                "size": 1
              }
            }
          }
        }
    });

    request["aggs"] = aggs;
    request.size(0);

    let response: JsonValue = ds.search(&request).await?.json().await?;

    let mut results = Vec::new();

    if let Some(buckets) = response["aggregations"]["client_mac"]["buckets"].as_array() {
        for bucket in buckets {
            let latest = &bucket["latest"]["hits"]["hits"][0]["_source"];
            results.push(latest);
        }
    }

    Ok(json!({
        "data": results,
    }))
}

pub async fn dhcp_report_request(
    ds: &EventStore,
    mut filters: Vec<JsonValue>,
) -> Result<JsonValue, DatastoreError> {
    let mut request = elastic::request::new_request();
    filters.push(elastic::request::term_filter("dhcp.dhcp_type", "request"));
    request.set_filters(filters);

    let aggs = json!({
        "client_mac": {
          "terms": {
            "field": "dhcp.client_mac.keyword",
            "size": 10000
          },
          "aggs": {
            "latest": {
              "top_hits": {
                "sort": [
                  {
                    "@timestamp": {
                      "order": "desc"
                    }
                  }
                ],
                "size": 1
              }
            }
          }
        }
    });

    request["aggs"] = aggs;
    request.size(0);

    let response: JsonValue = ds.search(&request).await?.json().await?;

    let mut results = Vec::new();

    if let Some(buckets) = response["aggregations"]["client_mac"]["buckets"].as_array() {
        for bucket in buckets {
            let latest = &bucket["latest"]["hits"]["hits"][0]["_source"];
            results.push(latest);
        }
    }

    Ok(json!({
        "data": results,
    }))
}

/// Return all IP addresses that appear to be DHCP servers.
pub async fn servers(
    ds: &EventStore,
    mut filters: Vec<JsonValue>,
) -> Result<JsonValue, DatastoreError> {
    let mut request = elastic::request::new_request();
    filters.push(elastic::request::term_filter("dhcp.type", "reply"));
    request.set_filters(filters);

    let aggs = json!({
        "servers": {
          "terms": {
            "field": "src_ip.keyword",
            "size": 10000
          },
        }
    });

    request["aggs"] = aggs;
    request.size(0);

    let response: JsonValue = ds.search(&request).await?.json().await?;

    let mut results = Vec::new();

    if let Some(buckets) = response["aggregations"]["servers"]["buckets"].as_array() {
        for bucket in buckets {
            let entry = json!({
                "ip": bucket["key"],
                "count": bucket["doc_count"],
            });
            results.push(entry);
        }
    }

    Ok(json!({
        "data": results,
    }))
}

/// For each client MAC address seen, return a list of IP addresses the MAC has
/// been assigned.
pub async fn mac(
    ds: &EventStore,
    mut filters: Vec<JsonValue>,
) -> Result<JsonValue, DatastoreError> {
    let mut request = elastic::request::new_request();
    filters.push(elastic::request::term_filter("dhcp.type", "reply"));
    request.set_filters(filters);

    let aggs = json!({
        "client_mac": {
          "terms": {
            "field": "dhcp.client_mac.keyword",
            "size": 10000
          },
          "aggs": {
            "assigned_ip": {
                "terms": {
                    "field": "dhcp.assigned_ip.keyword"
                }
            }
          }
        }
    });

    request["aggs"] = aggs;
    request.size(0);

    let response: JsonValue = ds.search(&request).await?.json().await?;

    let mut results = Vec::new();

    if let JsonValue::Array(buckets) = &response["aggregations"]["client_mac"]["buckets"] {
        for bucket in buckets {
            let mut addrs = Vec::new();
            if let JsonValue::Array(buckets) = &bucket["assigned_ip"]["buckets"] {
                for v in buckets {
                    if let JsonValue::String(v) = &v["key"] {
                        // Not really interested in 0.0.0.0.
                        if v != "0.0.0.0" {
                            addrs.push(v);
                        }
                    }
                }
            }

            let entry = json!({
                "mac": bucket["key"],
                "addrs": addrs,
            });
            results.push(entry);
        }
    }

    Ok(json!({
        "data": results,
    }))
}

/// For each assigned IP address, return a list of MAC addresses that have been
/// assigned that IP address.
pub async fn ip(ds: &EventStore, mut filters: Vec<JsonValue>) -> Result<JsonValue, DatastoreError> {
    let mut request = elastic::request::new_request();
    filters.push(elastic::request::term_filter("dhcp.type", "reply"));
    request.set_filters(filters);

    let aggs = json!({
        "assigned_ip": {
          "terms": {
            "field": "dhcp.assigned_ip.keyword",
            "size": 10000,
          },
          "aggs": {
            "client_mac": {
                "terms": {
                    "field": "dhcp.client_mac.keyword",
                }
            }
          }
        }
    });

    request["aggs"] = aggs;
    request.size(0);

    let response: JsonValue = ds.search(&request).await?.json().await?;

    let mut results = Vec::new();

    if let JsonValue::Array(buckets) = &response["aggregations"]["assigned_ip"]["buckets"] {
        for bucket in buckets {
            // Skip 0.0.0.0.
            // TODO: Filter out in the query.
            if bucket["key"] == JsonValue::String("0.0.0.0".to_string()) {
                continue;
            }

            let mut addrs = Vec::new();
            if let JsonValue::Array(buckets) = &bucket["client_mac"]["buckets"] {
                for v in buckets {
                    if let JsonValue::String(v) = &v["key"] {
                        addrs.push(v);
                    }
                }
            }

            let entry = json!({
                "ip": bucket["key"],
                "macs": addrs,
            });
            results.push(entry);
        }
    }

    Ok(json!({
        "data": results,
    }))
}
