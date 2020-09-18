module Model exposing (..)

import Browser.Navigation exposing (Key)
import Css
import Route exposing (Route)


type alias Flags =
    { width : Int
    , isDarkTheme : Bool
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


type alias Theme =
    { color : Css.Style
    , backgroundColor : Css.Style
    , borderColor : Css.Style
    }


type alias SearchInfo =
    { countHits : Int
    , countPages : Int
    , currentPage : Int
    }


initSearchInfo : SearchInfo
initSearchInfo =
    { countHits = 0
    , countPages = 1
    , currentPage = 1
    }


type alias Model =
    { navKey : Key
    , route : Route
    , theme : Theme
    , width : Int
    , algolia : Algolia
    , isFadein : IsFadein
    , searchInput : String
    , searchInfo : SearchInfo
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
