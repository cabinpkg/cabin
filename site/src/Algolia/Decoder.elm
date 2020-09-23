module Algolia.Decoder exposing (searchResponseDecoder)

import Json.Decode
import Json.Decode.Pipeline
import Model exposing (Package, SearchResponse)


searchResponseDecoder : Json.Decode.Decoder SearchResponse
searchResponseDecoder =
    Json.Decode.succeed SearchResponse
        |> Json.Decode.Pipeline.required "hits" searchHitListDecoder
        |> Json.Decode.Pipeline.required "nbHits" Json.Decode.int
        |> Json.Decode.Pipeline.required "nbPages" Json.Decode.int


searchHitListDecoder : Json.Decode.Decoder (List Package)
searchHitListDecoder =
    Json.Decode.list packageSearchHitDecoder


packageSearchHitDecoder : Json.Decode.Decoder Package
packageSearchHitDecoder =
    Json.Decode.succeed Package
        |> Json.Decode.Pipeline.requiredAt [ "package", "name" ] Json.Decode.string
        |> Json.Decode.Pipeline.requiredAt [ "_highlightResult", "package", "name", "value" ] Json.Decode.string
        |> Json.Decode.Pipeline.requiredAt [ "package", "version" ] Json.Decode.string
        |> Json.Decode.Pipeline.requiredAt [ "package", "description" ] Json.Decode.string
        |> Json.Decode.Pipeline.requiredAt [ "package", "repository" ] Json.Decode.string
