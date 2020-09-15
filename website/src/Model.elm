module Model exposing (..)

import Browser.Navigation exposing (Key)
import Route exposing (Route)


type alias Flags =
    { width : Int
    , algoliaApiKey : String
    , algoliaApplicationId : String
    , algoliaIndexName : String
    }


type alias Algolia =
    { apiKey : String
    , applicationId : String
    , indexName : String
    }


type alias IsFadein =
    { section1 : Bool
    , getStart : Bool
    }


type alias Model =
    { navKey : Key
    , route : Route
    , width : Int
    , algolia : Algolia
    , isFadein : IsFadein
    , searchInput : String
    }
