module Model exposing (..)

import Browser.Navigation exposing (Key)
import Route exposing (Route)


type alias Flags =
    { width : Int
    , algoliaApplicationId : String
    , algoliaApiKey : String
    , algoliaIndexName : String
    }


type alias Algolia =
    { applicationId : String
    , apiKey : String
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
    , searchInfo :
        { countHits : Int
        , countPages : Int
        , currentPage : Int
        }
    , packages : List Package
    }



-- Package


type alias SearchResponse =
    { hits : List Package
    , nbHits : Int
    , nbPages : Int
    }


type alias Package =
    { name : String
    , nameHighlighted : String
    , version : String
    , description : String
    , repository : String
    }
